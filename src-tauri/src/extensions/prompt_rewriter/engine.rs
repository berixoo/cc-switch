//! Pure, transactional full-text replacement. File identity requires a complete client envelope and
//! an absolute path resolving lexically to the selected file. The body must be the cached source text
//! or an older snapshot of that same file, which is what resumed conversations replay.
use super::adapters::{self, Selection};
use super::config::{AppendPosition, ApplyMode, Snapshot, MAX_TEXT_BYTES};
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

/// Combine the client's text with the configured text according to the target's mode.
fn apply(mode: ApplyMode, position: AppendPosition, original: &str, replacement: &str) -> String {
    match mode {
        ApplyMode::Replace => replacement.to_string(),
        ApplyMode::Append => {
            if replacement.is_empty() {
                return original.to_string();
            }
            if original.is_empty() {
                return replacement.to_string();
            }
            match position {
                AppendPosition::End => {
                    if original.ends_with('\n') {
                        format!("{original}{replacement}")
                    } else {
                        format!("{original}\n{replacement}")
                    }
                }
                AppendPosition::Start => {
                    // Leading header lines (billing markers, identity lines) are metadata the
                    // client relies on; insert after them so they stay first.
                    let split = leading_header_end(original);
                    let (head, body) = original.split_at(split);
                    if head.is_empty() {
                        format!("{replacement}\n{body}")
                    } else if body.is_empty() {
                        format!("{head}{replacement}")
                    } else {
                        format!("{head}{replacement}\n{body}")
                    }
                }
            }
        }
    }
}

/// Byte offset just past the run of leading `key: value` header lines, or 0 when the text does
/// not start with such lines.
fn leading_header_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let is_header = trimmed.split_once(':').is_some_and(|(key, _)| {
            !key.is_empty()
                && key.len() <= 64
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ')
        });
        if !is_header {
            break;
        }
        offset += line.len();
    }
    // Keep the blank separator that follows a header block with the header.
    if offset > 0 && text[offset..].starts_with('\n') {
        offset += 1;
    }
    offset
}

/// Remove the built-in safety notice from every system carrier in `body`.
fn strip_from_system(body: &mut Value, format: WireFormat) {
    let field = match format {
        WireFormat::Anthropic => ("system", false),
        WireFormat::Responses => ("instructions", true),
        WireFormat::Chat => ("messages", false),
        WireFormat::Gemini => ("systemInstruction", false),
    };
    match format {
        WireFormat::Chat => {
            let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
                return;
            };
            for message in messages.iter_mut() {
                if matches!(message["role"].as_str(), Some("system" | "developer")) {
                    strip_in_container(&mut message["content"]);
                }
            }
        }
        WireFormat::Gemini => {
            if let Some(parts) = body
                .get_mut("systemInstruction")
                .and_then(|value| value.get_mut("parts"))
            {
                strip_in_container(parts);
            }
        }
        _ => {
            let (name, _) = field;
            if let Some(slot) = body.get_mut(name) {
                strip_in_container(slot);
            }
        }
    }
}

/// Strip notices from a string-or-blocks container in place.
fn strip_in_container(container: &mut Value) {
    match container {
        Value::String(text) => {
            let (stripped, _) = super::safety_notice::strip_safety_notice(text);
            if stripped != *text {
                *text = stripped;
            }
        }
        Value::Array(blocks) => {
            for block in blocks.iter_mut() {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let (stripped, _) = super::safety_notice::strip_safety_notice(text);
                    if stripped != text {
                        block["text"] = Value::String(stripped);
                    }
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn rewrite(
    snapshot: &Snapshot,
    route: Route,
    format: WireFormat,
    source: &Source,
    body: Value,
) -> Result<Preview, String> {
    let strip_notice = snapshot.config.routes.get(route).system.strip_safety_notice;
    // Removing the built-in notice changes what the client sent, so do it once up front and
    // report it through the preview's before/after text.
    let mut body = body;
    if strip_notice {
        strip_from_system(&mut body, format);
    }
    let selection = adapters::select(&body, format)?;
    let mut spans: Vec<_> = selection
        .nodes
        .iter()
        .map(|node| envelopes(&node.text, source))
        .collect();
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
    let system_mode = config.system.mode;
    let system_position = config.system.position;
    let system = config
        .system
        .enabled
        .then_some(config.system.replacement.as_deref())
        .flatten();
    let user_mode = config.user_instructions.mode;
    let user_position = config.user_instructions.position;
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
    // Only replace in place. A client that does not send the global file gets no request-level
    // instruction message: anything added here becomes part of the client's conversation history
    // and keeps affecting later turns even after the extension is switched off.
    // Non-Anthropic formats may share one carrier between system and file text. When that
    // carrier exists but no file envelope was found, the file target has nothing to edit while
    // the system target still does: drop only the file target instead of cancelling the request.
    let shared_carrier_without_file = format != WireFormat::Anthropic
        && system.is_some()
        && user.is_some()
        && count == 0
        && selection.nodes.iter().any(|node| node.system);
    if system.is_some() && uncertain_system {
        preview.reason = Some(
            "system and file instruction boundaries cannot be separated; no changes applied".into(),
        );
        return Ok(preview);
    }
    let user = if shared_carrier_without_file {
        None
    } else {
        user
    };
    if user.is_some() && count == 0 {
        let any_envelope = spans.iter().flatten().next().is_some();
        preview.reason = Some(if user == Some("") {
            "replacement is empty; no user instruction applied".into()
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
                    if !inserted {
                        inserted = true;
                        apply(system_mode, system_position, chunk, replacement)
                    } else if offset > 0 && chunk.trim().is_empty() {
                        chunk.to_string()
                    } else {
                        String::new()
                    }
                } else {
                    chunk.to_string()
                }
            } else {
                chunk.to_string()
            };
            rewritten.push_str(&chunk);
            if node.system
                && system.is_some()
                && !rewritten.is_empty()
                && !rewritten.ends_with('\n')
            {
                rewritten.push('\n');
            }
            body_only.push_str(&chunk);
            if let Some(replacement) = user.filter(|_| targetable(span)) {
                let original = &node.text[span.content_start..span.content_end];
                rewritten.push_str(&node.text[span.start..span.content_start]);
                rewritten.push_str(&apply(user_mode, user_position, original, replacement));
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
                    apply(system_mode, system_position, suffix, replacement)
                } else {
                    String::new()
                }
            } else {
                suffix.to_string()
            }
        } else {
            suffix.to_string()
        };
        if node.system
            && system.is_some()
            && !spans.is_empty()
            && !suffix.is_empty()
            && !suffix.starts_with('\n')
        {
            rewritten.push('\n');
        }
        rewritten.push_str(&suffix);
        body_only.push_str(&suffix);
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
    if size.saturating_add(created.map_or(0, str::len)) > 8 * 1024 * 1024 {
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
    preview.user_applied = user.is_some() && count >= 1;
    preview.system_applied = system.is_some();
    // Report what actually goes on the wire. In append mode that is the client text plus ours,
    // so echoing only the configured text would misreport the result.
    let reported = after_system.join("\n");
    preview.system_after = if reported.trim().is_empty() {
        None
    } else {
        Some(reported)
    };
    Ok(preview)
}
