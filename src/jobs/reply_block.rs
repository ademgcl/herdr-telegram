//! Marked reply blocks: the one unambiguous way to separate the agent's
//! answer from its tool transcript.
//!
//! An agent TUI renders the whole turn into the pane — the assistant's
//! prose AND every command, diff and file it read. Nothing in that text
//! says which is which: a reply can contain `()`, a path or a `$`, and a
//! `grep` dump reads like English. Every shape-based attempt (prose
//! scoring, diff/`$` detection, block filtering) was measured against the
//! real pane and each one either passed a tool dump through or deleted a
//! genuine short reply.
//!
//! So the agent states it instead. It wraps its answer in
//! `[[reply]] … [[end]]`; the final card is exactly what is between the
//! markers, with no judgement applied. Turns WITHOUT markers (any other
//! agent, an older session, a plain "ok") keep the existing arbitration
//! untouched — this only ever replaces a body it can positively identify.
use crate::jobs::stream::join_trimmed;

/// Opening marker, alone on its line.
pub const REPLY_OPEN: &str = "[[reply]]";
/// Closing marker, alone on its line.
pub const REPLY_CLOSE: &str = "[[end]]";

fn is_marker(line: &str, marker: &str) -> bool {
    line.trim() == marker
}

/// The agent's marked answer in these lines, if present.
///
/// The LAST complete pair wins: an agent may mark a mid-turn status and
/// then answer again, and the final card is the last thing it said.
pub fn marked_reply(lines: &[String]) -> Option<String> {
    let mut found: Option<String> = None;
    let mut open: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if open.is_none() {
            if is_marker(l, REPLY_OPEN) {
                open = Some(i + 1);
            }
        } else if is_marker(l, REPLY_CLOSE) {
            let start = open.take().unwrap_or(0);
            let body = join_trimmed(&lines[start..i]);
            if !body.is_empty() {
                found = Some(body);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{REPLY_CLOSE, REPLY_OPEN, marked_reply};
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_marked_block_is_extracted_verbatim() {
        let lines = v(&[
            "  ┃  $ cargo test 2>&1 | tail -12",
            "  ┃  test result: ok. 540 passed; 0 failed",
            REPLY_OPEN,
            "Fixed the parser — the card is the reply now.",
            "Tool output never reaches Telegram again.",
            REPLY_CLOSE,
            "   ┃  esc interrupt",
        ]);
        let body = marked_reply(&lines).expect("marked reply");
        assert!(body.contains("Fixed the parser"));
        assert!(body.contains("Tool output never reaches Telegram"));
        for junk in ["cargo test", "540 passed", "esc interrupt", "[[", "┃"] {
            assert!(!body.contains(junk), "leaked {junk:?}: {body:?}");
        }
    }

    /// The live pane's real ordering: prose, then the tool calls it ran,
    /// then the marked answer last. A positional picker takes the tool
    /// block; this takes the marked one.
    #[test]
    fn test_last_marked_block_wins_over_earlier_ones() {
        let lines = v(&[
            REPLY_OPEN,
            "interim status, superseded",
            REPLY_CLOSE,
            "   ┃  $ herdr pane read wZ:p1",
            REPLY_OPEN,
            "the answer that counts",
            REPLY_CLOSE,
        ]);
        assert_eq!(
            marked_reply(&lines).as_deref(),
            Some("the answer that counts")
        );
    }

    /// No markers (any other agent, or a plain "ok") must change nothing.
    #[test]
    fn test_unmarked_turn_yields_nothing() {
        let lines = v(&["ok", "  ┃  $ ls -la", "test result: ok. 540 passed"]);
        assert_eq!(marked_reply(&lines), None);
    }

    /// An empty marked block is not a reply — the turn may have died
    /// between the markers. Falling back beats posting nothing.
    #[test]
    fn test_empty_block_is_not_a_reply() {
        let lines = v(&[REPLY_OPEN, REPLY_CLOSE]);
        assert_eq!(marked_reply(&lines), None);
    }

    /// Prose that itself contains marker-shaped text must not confuse the
    /// scan: only a marker ALONE on its line counts.
    #[test]
    fn test_marker_must_be_alone_on_its_line() {
        let lines = v(&["see the [[end]] convention, e.g. [[reply]] then prose"]);
        assert_eq!(marked_reply(&lines), None);
    }
}
