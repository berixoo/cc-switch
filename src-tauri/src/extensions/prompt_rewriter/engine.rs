//! Pure, transactional full-text replacement. File identity requires a complete client envelope and
//! an absolute path resolving lexically to the selected file. The body must be the cached source text
//! or an older snapshot of that same file, which is what resumed conversations replay.
use super::adapters::{self, Selection};
use super::config::{Snapshot, MAX_TEXT_BYTES};
use super::{Route, Source, SourceState, WireFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    #[default]
    Unknown,
    Unidentified,
    Ambiguous,
    Identified,
    Injected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub body: Value,
    pub system_before: Option<String>,
    pub system_after: Option<String>,
    pub file_status: FileStatus,
    pub system_applied: bool,
    pub user_applied: bool,
    pub reason: Option<String>,
}

#[derive(Debug)]
struct Envelope {
    start: usize,
    end: usize,
    content_start: usize,
    content_end: usize,
    matched: bool,
    source_path_matched: bool,
    /// Absolute path matched and the envelope is well formed, but its body is an older
    /// snapshot of the same file. Resumed conversations replay the text captured when the
    /// session started, so exact body equality cannot be required there.
    historical: bool,
}

/// A segment carrying this file's instructions, current or from an earlier session.
fn targetable(span: &Envelope) -> bool {
    span.matched || span.historical
}

/// Lexical comparison only: no IO, no relative paths, no traversal or guessed home expansion.
fn normalized_path(path: &str) -> Option<String> {
    if path.chars().any(char::is_control) {
        return None;
    }
    let drive = path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && matches!(path.as_bytes().get(2), Some(b'/' | b'\\'));
    let windows = drive || path.starts_with("\\\\") || path.starts_with("//");
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    if !path.starts_with('/') && !drive {
        return None;
    }
    if path.split('/').any(|part| part == "." || part == "..") {
        return None;
    }
    let path = path.trim_end_matches('/').to_string();
    Some(if drive || path.starts_with("//") {
        path.to_ascii_lowercase()
    } else {
        path
    })
}

fn same_path(left: &str, right: &str) -> bool {
    match (normalized_path(left), normalized_path(right)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn envelopes(text: &str, source: &Source) -> Vec<Envelope> {
    let prefix = "<system-reminder>\n";
    let header_end = " (user's private global instructions for all projects):\n\n";
    let close = "\n</system-reminder>";
    const PREAMBLE_LIMIT: usize = 1024;
    let mut result = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(prefix) {
        let start = offset + relative;
        // A wrapper must start at a line boundary, never inside a quoted heading.
        if start > 0 && text.as_bytes()[start - 1] != b'\n' {
            offset = start + prefix.len();
            continue;
        }
        let path_start = {
            let body_start = start + prefix.len();
            if text[body_start..].starts_with("Contents of ") {
                body_start + "Contents of ".len()
            } else {
                // Clients may put a short, tag-free preamble inside the same wrapper (a context
                // heading, a "Codebase and user instructions..." sentence, or both). Allow a
                // bounded one; anything containing markup is treated as an unknown wrapper.
                let mut window_end = (body_start + PREAMBLE_LIMIT).min(text.len());
                while !text.is_char_boundary(window_end) {
                    window_end -= 1;
                }
                let window = &text[body_start..window_end];
                match window.find("Contents of ") {
                    Some(offset) if !window[..offset].contains('<') => {
                        body_start + offset + "Contents of ".len()
                    }
                    _ => {
                        offset = body_start;
                        continue;
                    }
                }
            }
        };
        let Some(header_length) = text[path_start..].find(header_end) else {
            break;
        };
        let path_end = path_start + header_length;
        let path = &text[path_start..path_end];
        if normalized_path(path).is_none() {
            offset = path_end + header_end.len();
            continue;
        }
        let content_start = path_end + header_end.len();
        let Some(content_length) = text[content_start..].find(close) else {
            break;
        };
        let content_end = content_start + content_length;
        let end = content_end + close.len();
        let content = &text[content_start..content_end];
        // Nested/partial envelopes are not a separable instruction segment.
        if content.contains(prefix) || (end < text.len() && text.as_bytes()[end] != b'\n') {
            offset = end;
            continue;
        }
        let source_path_matched = source
            .path
            .as_deref()
            .is_some_and(|source_path| same_path(path, source_path));
        let matched = source.state == SourceState::Ready
            && source_path_matched
            && source.content.as_deref().is_some_and(|original| {
                // Accept an exact empty envelope too, but never normalize a nonempty source
                // into an empty match. Claude may omit one terminal LF.
                original == content
                    || (!content.is_empty() && original.strip_suffix('\n') == Some(content))
            });
        let historical = source.state == SourceState::Ready
            && source.content.is_some()
            && source_path_matched
            && !matched;
        result.push(Envelope {
            start,
            end,
            content_start,
            content_end,
            matched,
            source_path_matched,
            historical,
        });
        offset = end;
    }
    result
}

fn outside(text: &str, spans: &[Envelope]) -> String {
    let mut output = String::new();
    let mut offset = 0;
    for span in spans {
        output.push_str(&text[offset..span.start]);
        offset = span.end;
    }
    output.push_str(&text[offset..]);
    output
}

fn system_text(
    selection: &Selection,
    spans: &[Vec<Envelope>],
    limit: usize,
) -> Result<Option<String>, String> {
    let mut result = String::new();
    let mut present = false;
    for (node, spans) in selection
        .nodes
        .iter()
        .zip(spans)
        .filter(|(node, _)| node.system)
    {
        let text = outside(&node.text, spans);
        let additional = text.len().saturating_add(usize::from(present));
        if result.len().saturating_add(additional) > limit {
            return Err("prompt rewriter: system text exceeds capture/preview size limit".into());
        }
        if present {
            result.push('\n');
        }
        result.push_str(&text);
        present = true;
    }
    Ok(present.then_some(result))
}

pub(crate) fn inspect_system(
    format: WireFormat,
    source: &Source,
    body: &Value,
) -> Result<Option<String>, String> {
    let selection = adapters::select(body, format)?;
    let spans: Vec<_> = selection
        .nodes
        .iter()
        .map(|node| envelopes(&node.text, source))
        .collect();
    system_text(&selection, &spans, MAX_TEXT_BYTES)
}

pub(crate) fn rewrite(
    snapshot: &Snapshot,
    route: Route,
    format: WireFormat,
    source: &Source,
    body: Value,
) -> Result<Preview, String> {
    let selection = adapters::select(&body, format)?;
    let mut spans: Vec<_> = selection
        .nodes
        .iter()
        .map(|node| envelopes(&node.text, source))
        .collect();
    // A stale envelope is not permission to add a second copy, even if it later fails
    // the stricter whole-message checks below.
    let source_envelope_present = spans.iter().flatten().any(|span| span.source_path_matched);
    // Request-level injection needs a loaded source at a real absolute path. Whether that source
    // is empty or already has text only changes which policy applies, not whether the current
    // replacement may be added when the client did not send the envelope at all.
    let loaded_source = source.state == SourceState::Ready
        && source.content.is_some()
        && source.path.as_deref().and_then(normalized_path).is_some();
    // User-role carriers must consist entirely of envelopes, not prose quoting a file envelope.
    for (node, spans) in selection.nodes.iter().zip(&mut spans) {
        if !node.system && !outside(&node.text, spans).trim().is_empty() {
            spans.clear();
        }
    }
    // A dedicated instruction message cannot contain prose in another text block either.
    let invalid_messages: std::collections::HashSet<_> = selection
        .nodes
        .iter()
        .zip(&spans)
        .filter(|(node, spans)| !outside(&node.text, spans).trim().is_empty())
        .filter_map(|(node, _)| node.message.as_deref())
        .collect();
    for (node, spans) in selection.nodes.iter().zip(&mut spans) {
        if node
            .message
            .as_deref()
            .is_some_and(|message| invalid_messages.contains(message))
        {
            spans.clear();
        }
    }
    let count = spans
        .iter()
        .flatten()
        .filter(|span| targetable(span))
        .count();
    // Identity is the absolute path plus a complete envelope. Resumed conversations replay the
    // snapshot taken when the session started, and clients may attach it more than once, so
    // neither exact body equality nor a single occurrence is required.
    let file_status = if count == 0 {
        FileStatus::Unidentified
    } else {
        FileStatus::Identified
    };
    let before = system_text(&selection, &spans, 8 * 1024 * 1024)?;
    let mut preview = Preview {
        body,
        system_before: before.clone(),
        system_after: before,
        file_status,
        system_applied: false,
        user_applied: false,
        reason: None,
    };
    if !snapshot.active(route) {
        preview.reason = Some("extension or route disabled".into());
        return Ok(preview);
    }
    let config = snapshot.config.routes.get(route);
    let system = config
        .system
        .enabled
        .then_some(config.system.replacement.as_deref())
        .flatten();
    let user = config
        .user_instructions
        .enabled
        .then_some(config.user_instructions.replacement.as_deref())
        .flatten();
    if system.is_none() && user.is_none() {
        return Ok(preview);
    }
    // Partial wrappers and unwrapped copies sharing a system carrier have no safe target split.
    let uncertain_system = selection.nodes.iter().zip(&spans).any(|(node, spans)| {
        if !node.system {
            return false;
        }
        let remaining = outside(&node.text, spans);
        // Claude's native system text can mention `<system-reminder>` inline as documentation.
        // Actual/partial boundaries (including a stray close) still require a safe split.
        ["<system-reminder>", "</system-reminder>"]
            .iter()
            .any(|tag| {
                remaining.contains(&format!("{tag}\n"))
                    || remaining.contains(&format!("{tag}\r\n"))
                    || remaining
                        .lines()
                        .any(|line| line.trim_start().starts_with(tag))
            })
            || source
                .content
                .as_deref()
                .filter(|text| !text.is_empty())
                .is_some_and(|text| {
                    remaining.contains(text)
                        || text
                            .strip_suffix('\n')
                            .filter(|text| !text.is_empty())
                            .is_some_and(|text| remaining.contains(text))
                })
    });
    let incomplete_file_carrier = selection.nodes.iter().zip(&spans).any(|(node, spans)| {
        !node.system
            && node.text.contains("Contents of ")
            && (spans.is_empty() || !outside(&node.text, spans).trim().is_empty())
    });
    let rejected_file_carrier = selection.rejected_file_carriers.iter().any(|text| {
        let spans = envelopes(text, source);
        spans.iter().any(|span| span.source_path_matched)
            || (text.contains("Contents of ")
                && (spans.is_empty() || !outside(text, &spans).trim().is_empty()))
    });
    // The client may not send the global file at all (resumed or compacted sessions, subagents).
    // Then the only way to apply the replacement is a request-level instruction message. It is
    // skipped whenever any carrier for this path exists in any form, so nothing is duplicated.
    let instructions_absent = loaded_source
        && !source_envelope_present
        && !incomplete_file_carrier
        && !rejected_file_carrier;
    let injected = user
        .filter(|text| instructions_absent && !text.is_empty())
        .map(|text| {
            format!("<cc-switch-user-instructions>\n{text}\n</cc-switch-user-instructions>")
        });
    // Anthropic's system is independent of an absent file. With a verified empty source,
    // Responses/Chat can also replace system text without inventing a file-content match.
    let unresolved_shared = format != WireFormat::Anthropic
        && system.is_some()
        && user.is_some()
        && count == 0
        && !instructions_absent
        && selection.nodes.iter().any(|node| node.system);
    if ((system.is_some() || injected.is_some()) && uncertain_system) || unresolved_shared {
        preview.reason = Some(
            "system and file instruction boundaries cannot be separated; no changes applied".into(),
        );
        return Ok(preview);
    }
    if user.is_some() && count == 0 && injected.is_none() {
        let any_envelope = spans.iter().flatten().next().is_some();
        preview.reason = Some(if user == Some("") {
            "replacement is empty; no user instruction added".into()
        } else if any_envelope {
            "instruction envelope found for a different path than the selected source; no user replacement applied".into()
        } else {
            "no instruction envelope for the selected source was found in the request; no user replacement applied".into()
        });
    }
    let mut texts = Vec::with_capacity(selection.nodes.len());
    let mut inserted = false;
    let mut after_system = Vec::new();
    for (node, spans) in selection.nodes.iter().zip(&spans) {
        let mut rewritten = String::new();
        let mut body_only = String::new();
        let mut offset = 0;
        for span in spans {
            let chunk = &node.text[offset..span.start];
            let chunk = if node.system {
                if let Some(replacement) = system {
                    if !inserted && !chunk.is_empty() {
                        inserted = true;
                        replacement
                    } else if offset > 0 && chunk.trim().is_empty() {
                        chunk
                    } else {
                        ""
                    }
                } else {
                    chunk
                }
            } else {
                chunk
            };
            rewritten.push_str(chunk);
            if node.system
                && system.is_some()
                && !rewritten.is_empty()
                && !rewritten.ends_with('\n')
            {
                rewritten.push('\n');
            }
            body_only.push_str(chunk);
            if let Some(replacement) = user.filter(|_| targetable(span)) {
                rewritten.push_str(&node.text[span.start..span.content_start]);
                rewritten.push_str(replacement);
                rewritten.push_str(&node.text[span.content_end..span.end]);
            } else {
                rewritten.push_str(&node.text[span.start..span.end]);
            }
            offset = span.end;
        }
        let suffix = &node.text[offset..];
        let suffix = if node.system {
            if let Some(replacement) = system {
                if !inserted && (!suffix.is_empty() || spans.is_empty()) {
                    inserted = true;
                    replacement
                } else {
                    ""
                }
            } else {
                suffix
            }
        } else {
            suffix
        };
        if node.system
            && system.is_some()
            && !spans.is_empty()
            && !suffix.is_empty()
            && !suffix.starts_with('\n')
        {
            rewritten.push('\n');
        }
        rewritten.push_str(suffix);
        body_only.push_str(suffix);
        if node.system {
            after_system.push(body_only);
        }
        texts.push(rewritten);
    }
    if let Some(replacement) = system.filter(|_| !inserted) {
        if let Some(index) = selection.nodes.iter().position(|node| node.system) {
            // A carrier containing only protected file envelopes still accepts a disjoint system
            // prefix; do not replace that carrier or discard its envelopes to create a new one.
            if !replacement.is_empty() {
                texts[index] = format!("{replacement}\n{}", texts[index]);
            }
            inserted = true;
        }
    }
    let created = system.filter(|_| !inserted);
    let size: usize = texts.iter().map(String::len).sum();
    if size
        .saturating_add(created.map_or(0, str::len))
        .saturating_add(injected.as_ref().map_or(0, String::len))
        > 8 * 1024 * 1024
    {
        return Err("prompt rewriter: rewritten text exceeds 8 MiB".into());
    }
    // Build and commit the entire edit plan in a private copy; no partial edits can escape.
    let changed = selection
        .nodes
        .iter()
        .zip(&texts)
        .any(|(node, text)| &node.text != text)
        || created.is_some_and(|text| !text.is_empty());
    if changed || system.is_some() {
        // preview.body is already the caller's private copy; errors discard this entire value.
        adapters::commit(
            &mut preview.body,
            format,
            &selection,
            &texts,
            created,
            system.is_some(),
        )?;
    }
    if let Some(text) = injected {
        adapters::prepend_user_instructions(&mut preview.body, format, &text)?;
        preview.file_status = FileStatus::Injected;
        preview.user_applied = true;
    } else {
        preview.user_applied = user.is_some() && count >= 1;
    }
    preview.system_applied = system.is_some();
    if let Some(replacement) = system {
        // Full replacement is one logical body even when original system text spanned carriers.
        preview.system_after = if replacement.is_empty() {
            None
        } else {
            Some(replacement.to_string())
        };
    } else if !after_system.is_empty() {
        preview.system_after = Some(after_system.join("\n"));
    }
    Ok(preview)
}
