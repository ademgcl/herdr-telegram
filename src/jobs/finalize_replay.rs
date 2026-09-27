//! Final-body replay guard (split from `finalize`: 300-line file limit).
//!
//! A wide two-column TUI (Kilo) glues a churning sidebar — model name,
//! `Steps`/`Cost`, token counts, modified files, `esc interrupt` — onto
//! the SAME screen lines as the conversation. Consecutive reads therefore
//! never match line-for-line, the delta fallback cut lands above the new
//! output, and the PREVIOUS turn's reply is re-served as this turn's
//! final (the live report: turn 2's card was turn 1's answer).
//!
//! The guard is deliberately fail-closed on replays and permissive on
//! new output: a body whose every content line is already in the last
//! DELIVERED snapshot rendered nothing new, so it must never post. A
//! genuine verbatim repeat is indistinguishable from a replay — and
//! re-posting the old answer is the worse failure.
use crate::jobs::arbitrate::STREAM_MIN_CHARS;

/// Min content chars for a line to count as delivered when comparing
/// prefixes. Below this a short generic line ("ok", "Model") would match
/// half the screen and suppress a real reply.
const MIN_PREFIX: usize = 16;

/// Does `line` already appear in the delivered snapshot?
///
/// Prefix-tolerant, not exact: the sidebar suffix churns every tick, so
/// the same sentence reads back with a different tail. A delivered line
/// that starts with this line (or vice versa) is the same content.
fn line_delivered(line: &str, delivered: &[String]) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    delivered.iter().any(|d| {
        let d = d.trim();
        d == t || (t.chars().count() >= MIN_PREFIX && (d.starts_with(t) || t.starts_with(d)))
    })
}

/// Is this body just the already-delivered answer re-served?
///
/// Every content line already present in the last delivered snapshot
/// means nothing new rendered this turn. Short bodies are exempt (a
/// reply like "ok" must still post) — `STREAM_MIN_CHARS` parity with
/// `stream::is_stale_body`, which is the same rule for the stream side.
pub fn delivered_replay(body: &str, delivered: &[String]) -> bool {
    if delivered.is_empty() || body.trim().is_empty() {
        return false;
    }
    if body.chars().count() < STREAM_MIN_CHARS {
        return false;
    }
    let mut content = 0;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        content += 1;
        if !line_delivered(line, delivered) {
            return false;
        }
    }
    content > 0
}

#[cfg(test)]
mod tests {
    use super::delivered_replay;

    /// Turn 1's screen as anchored when its final landed: a Kilo pane
    /// whose sidebar already carries live counters.
    fn delivered() -> Vec<String> {
        vec![
            "> hi from tell".to_string(),
            "┃ Got it — and the log shows the new path active:".to_string(),
            "┃ [live] post wZ:p1 m2964 ok".to_string(),
            "┃ That's a newly minted transient (m2964), not a reuse.".to_string(),
            "┃  Kilo Gateway          Model      Steps      Cost    204     $0.00".to_string(),
        ]
    }

    /// The body the live bot posted as turn 2's final. The sidebar tail
    /// differs from the snapshot (counters moved) — exactly the churn
    /// that defeated the line-exact anchor.
    const LEAKED: &str = "┃ Got it — and the log shows the new path active:\n\
        ┃ [live] post wZ:p1 m2964 ok\n\
        ┃ That's a newly minted transient (m2964), not a reuse.";

    #[test]
    fn test_catches_the_reported_replay_despite_sidebar_churn() {
        // Every content line is turn 1's, only the sidebar tail moved.
        // Must be recognised so it never becomes turn 2's final.
        let churned = LEAKED.replace("m2964 ok", "m2964 ok          Kilo Gateway   262.4K (26%)");
        assert!(delivered_replay(&churned, &delivered()));
    }

    #[test]
    fn test_fresh_answer_is_never_a_replay() {
        let fresh = "Here is the second answer you asked for, in full.";
        assert!(!delivered_replay(fresh, &delivered()));
    }

    #[test]
    fn test_partially_new_body_posts() {
        // One fresh line is enough to make the body this turn's answer.
        let mixed = format!("{LEAKED}\n┃ And here is the genuinely new second answer.");
        assert!(!delivered_replay(&mixed, &delivered()));
    }

    #[test]
    fn test_short_reply_still_posts() {
        // Terse replies must never be suppressed forever ("ok" parity).
        assert!(!delivered_replay("ok", &delivered()));
    }

    #[test]
    fn test_no_delivered_snapshot_never_suppresses() {
        // First turn on a pane: nothing was delivered, so nothing can be
        // a replay.
        assert!(!delivered_replay(LEAKED, &[]));
    }
}
