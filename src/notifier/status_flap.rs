//! Done↔idle flap collapse (split from `status`: 300-line file limit).
use std::time::{Duration, Instant};

/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions.
pub(crate) const FLAP_WINDOW_SECS: u64 = 15;

/// Pure verdict (tested): collapse rapid done↔idle oscillation up front.
/// Time-bounded: the caller leaves last_change untouched on collapse, so
/// a perpetual fast flap goes quiet for at most one window, never forever.
/// Slow sampled bounces are legitimate completions.
pub(crate) fn collapse_flap(
    old: Option<&str>,
    new_status: &str,
    prev_change: Option<Instant>,
) -> bool {
    ((old == Some("done") && new_status == "idle") || (old == Some("idle") && new_status == "done"))
        && prev_change
            .map(|t| t.elapsed() < Duration::from_secs(FLAP_WINDOW_SECS))
            .unwrap_or(false)
}

/// Baseline-anchor verdict (single source for both suppression arms):
/// an outage/empty screen must never wipe the baseline — anchoring it
/// reposts scrollback as fresh on the next tick. A cleared pane reads as
/// non-empty all-blank lines and counts the same (anchor_baseline parity
/// in jobs::job — anchoring it makes the whole next screen "fresh").
/// Pure for tests. Delegates to [`crate::types::anchorable_screen`] (one
/// predicate for every `seen` anchor — dup'd copies re-drift).
pub(crate) fn should_anchor_baseline(screen: &[String]) -> bool {
    crate::types::anchorable_screen(screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_flap_fast_only() {
        let now = Instant::now();
        let recent = Some(now);
        let old_t = Some(now - Duration::from_secs(FLAP_WINDOW_SECS + 5));
        // Fast bounces collapse both directions.
        assert!(collapse_flap(Some("done"), "idle", recent));
        assert!(collapse_flap(Some("idle"), "done", recent));
        // Slow bounces are legitimate completions.
        assert!(!collapse_flap(Some("done"), "idle", old_t));
        assert!(!collapse_flap(Some("idle"), "done", old_t));
        // Non-flap pairs never collapse, even fast.
        assert!(!collapse_flap(Some("working"), "idle", recent));
        assert!(!collapse_flap(Some("done"), "blocked", recent));
        assert!(!collapse_flap(None, "idle", recent));
    }

    #[test]
    fn test_should_anchor_baseline_never_empty() {
        // An outage/empty screen must never wipe the baseline, or the
        // next tick reposts scrollback as fresh work.
        assert!(!should_anchor_baseline(&[]));
        assert!(should_anchor_baseline(&["out".to_string()]));
        // A cleared pane (all-blank lines) is outage, not content —
        // anchoring it wipes a good baseline the same way.
        assert!(!should_anchor_baseline(&[
            "".to_string(),
            "   ".to_string()
        ]));
        assert!(should_anchor_baseline(&["".to_string(), "out".to_string()]));
    }
}
