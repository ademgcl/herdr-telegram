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

/// The marked answer for THIS turn only.
///
/// A whole-screen scan is not enough: the settled screen still holds the
/// PREVIOUS turn's markers in its scrollback, so an unscoped search
/// re-posted last turn's marked block as this turn's card (the live
/// report: turn 2's final was turn 1's `[[reply]]` body). Scoping starts
/// after the last prompt echo — the same echo rule the final card's
/// segmentation uses — and falls back to the whole buffer when no echo
/// is visible (alt-screen panes often scroll it away), which keeps a
/// marked reply reachable rather than silently dropped.
pub fn marked_reply_for_turn(lines: &[String], prompt: &str) -> Option<String> {
    let want = prompt.lines().next().map(str::trim).unwrap_or("");
    let start = if want.is_empty() {
        0
    } else {
        lines
            .iter()
            .rposition(|l| crate::jobs::echo::is_prompt_echo(l, want))
            .map(|i| i + 1)
            .unwrap_or(0)
    };
    marked_reply(&lines[start..])
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

#[cfg(test)]
mod turn_scope_tests {
    use super::{REPLY_CLOSE, REPLY_OPEN, marked_reply, marked_reply_for_turn};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// The live failure: turn 1's markers are still in the screen
    /// scrollback when turn 2 finalizes, and the unscoped scan handed
    /// turn 1's body back as turn 2's card.
    #[test]
    fn test_previous_turns_markers_never_become_this_turns_card() {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let prompt = format!("second message {n}");
        let lines = v(&[
            "  ┃  first message",
            REPLY_OPEN,
            "turn one answer, already delivered",
            REPLY_CLOSE,
            "  ┃  $ cargo test",
            "  ┃  test result: ok. 540 passed",
            &format!("  ┃  {prompt}"),
            REPLY_OPEN,
            "turn two answer, the one that belongs here",
            REPLY_CLOSE,
        ]);
        // Unscoped: last pair still wins, but only because turn 2 marked
        // one. With turn 2 silent the old pair would answer for it.
        assert_eq!(
            marked_reply(&lines).as_deref(),
            Some("turn two answer, the one that belongs here")
        );
        assert_eq!(
            marked_reply_for_turn(&lines, &prompt).as_deref(),
            Some("turn two answer, the one that belongs here")
        );
    }

    /// Turn 2 produced no marked reply: turn 1's block must NOT be reused
    /// (that was the reported wrong final), and the caller falls back to
    /// the normal arbitration.
    #[test]
    fn test_unmarked_turn_does_not_inherit_the_previous_markers() {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let prompt = format!("second message {n}");
        let lines = v(&[
            "  ┃  first message",
            REPLY_OPEN,
            "turn one answer, already delivered",
            REPLY_CLOSE,
            &format!("  ┃  {prompt}"),
            "  ┃  $ cargo test",
        ]);
        assert_eq!(marked_reply_for_turn(&lines, &prompt), None);
    }

    /// No visible echo (alt-screen panes scroll it away): scan the whole
    /// buffer rather than silently losing a real marked reply.
    #[test]
    fn test_missing_echo_still_finds_the_marked_reply() {
        let lines = v(&["  ┃  $ cargo test", REPLY_OPEN, "the answer", REPLY_CLOSE]);
        assert_eq!(
            marked_reply_for_turn(&lines, "a prompt that scrolled away").as_deref(),
            Some("the answer")
        );
    }
}
