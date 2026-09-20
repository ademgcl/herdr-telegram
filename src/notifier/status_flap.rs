//! Done↔idle flap collapse (split from `status`: 300-line file limit).
use std::time::{Duration, Instant};

/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions.
pub(crate) const FLAP_WINDOW_SECS: u64 = 15;

/// Pure verdict (tested): collapse rapid done↔idle oscillation up front —
/// a perpetual fast flap suppresses forever by design (it never did work
/// between samples). Slow sampled bounces are legitimate completions.
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
/// reposts scrollback as fresh on the next tick. Pure for tests.
pub(crate) fn should_anchor_baseline(screen: &[String]) -> bool {
    !screen.is_empty()
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
    }
}
