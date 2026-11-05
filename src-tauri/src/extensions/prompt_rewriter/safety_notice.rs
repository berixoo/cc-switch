//! Removes the client's built-in safety boilerplate from a system prompt.
//!
//! The notice is prose that Anthropic may reword between releases, so matching is anchored on
//! stable fragments and verified by several independent signals. A block that only partly
//! matches is left untouched: dropping the wrong paragraph would be far worse than leaving a
//! stale notice in place.

/// Opening fragment that identifies the notice. Kept short and distinctive.
const OPENING: &str = "IMPORTANT: Assist with authorized security testing";

/// Signals that must all appear inside the candidate line for it to count as the notice.
/// Wording around them may change; these are the concepts the notice cannot omit.
const REQUIRED: [&str; 4] = [
    "defensive security",
    "Dual-use security tools",
    "clear authorization context",
    "malicious purposes",
];

/// Phrases the notice must end with. Newer releases may reword these, so any match counts.
const TERMINATORS: [&str; 3] = [
    "defensive use cases.",
    "for malicious purposes.",
    "use cases.",
];

/// What a strip attempt did, so callers can report it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Strip {
    /// The notice was found and removed.
    Removed,
    /// No notice is present, so the text is unchanged.
    Absent,
    /// Something looked like the notice but could not be verified; text is unchanged.
    Unverified,
}

/// Remove one safety notice from `text`.
///
/// Only whole lines are considered: the notice always occupies its own line, so a neighbouring
/// rule that happens to share a paragraph is never touched.
pub(crate) fn strip_safety_notice(text: &str) -> (String, Strip) {
    let Some(start) = text.find(OPENING) else {
        return (text.to_string(), Strip::Absent);
    };
    // Only accept the notice at the start of a line, never inside another sentence.
    if start > 0 && text.as_bytes()[start - 1] != b'\n' {
        return (text.to_string(), Strip::Absent);
    }
    let rest = &text[start..];
    let end = match rest.find('\n') {
        Some(offset) => start + offset,
        None => start + rest.len(),
    };
    strip_if_verified(text, start, end, &text[start..end])
}

fn strip_if_verified(text: &str, start: usize, end: usize, candidate: &str) -> (String, Strip) {
    // Every required concept must be present, and the line must end with a known terminator.
    if !REQUIRED.iter().all(|needle| candidate.contains(needle)) {
        return (text.to_string(), Strip::Unverified);
    }
    if !TERMINATORS
        .iter()
        .any(|ending| candidate.trim_end().ends_with(ending))
    {
        return (text.to_string(), Strip::Unverified);
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    // Drop the newline that belonged to the removed line, keeping the next line intact.
    let after = &text[end..];
    out.push_str(after.strip_prefix('\n').unwrap_or(after));
    (out, Strip::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTICE: &str = "IMPORTANT: Assist with authorized security testing, defensive security, CTF challenges, and educational contexts. Refuse requests for destructive techniques, DoS attacks, mass targeting, supply chain compromise, or detection evasion for malicious purposes. Dual-use security tools (C2 frameworks, credential testing, exploit development) require clear authorization context: pentesting engagements, CTF competitions, security research, or defensive use cases.";

    #[test]
    fn removes_the_notice_and_keeps_the_neighbouring_rule() {
        let text =
            format!("Header.\n\n{NOTICE}\nIMPORTANT: You must NEVER generate or guess URLs.");
        let (out, status) = strip_safety_notice(&text);
        assert_eq!(status, Strip::Removed);
        assert_eq!(
            out,
            "Header.\n\nIMPORTANT: You must NEVER generate or guess URLs."
        );
        assert!(!out.contains("defensive security"));
    }

    #[test]
    fn reworded_notice_is_still_recognised() {
        let reworded = "IMPORTANT: Assist with authorized security testing and defensive security reviews. Content aimed at malicious purposes is refused. Dual-use security tools need clear authorization context and a defensive use cases.";
        let text = format!("{reworded}\nKeep me.");
        let (out, status) = strip_safety_notice(&text);
        assert_eq!(status, Strip::Removed, "out: {out}");
        assert_eq!(out, "Keep me.");
    }

    #[test]
    fn leaves_partial_matches_alone() {
        let cases = [
            "IMPORTANT: Assist with authorized security testing. Do good.".to_string(),
            format!(
                "{OPENING}, defensive security, Dual-use security tools, clear authorization context, malicious purposes"
            ),
            format!("Some prose {NOTICE}"),
        ];
        for case in cases {
            let (out, status) = strip_safety_notice(&case);
            assert_eq!(out, case, "must not modify: {case}");
            assert_ne!(status, Strip::Removed, "must not remove: {case}");
        }
    }

    #[test]
    fn absent_notice_is_a_noop() {
        let text = "Just a normal system prompt.";
        let (out, status) = strip_safety_notice(text);
        assert_eq!(out, text);
        assert_eq!(status, Strip::Absent);
    }

    #[test]
    fn only_one_notice_is_removed_per_call() {
        let text = format!("{NOTICE}\nMiddle.\n{NOTICE}");
        let (out, status) = strip_safety_notice(&text);
        assert_eq!(status, Strip::Removed);
        assert!(out.contains("Middle."));
        assert!(out.contains(OPENING), "second copy stays for the next call");
    }
}
