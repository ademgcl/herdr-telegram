//! Pure stall-alert decisions shared by the watcher and watchdog paths.
//! Split from `limits` (300-line file limit): predicates here are
//! unit-tested, the async RPC/send orchestration stays in `limits`/`stall`.
use std::time::{Duration, Instant};

/// How many recent lines count as "fresh" for settled panes: a quota
/// banner only in deep scrollback under idle/done is a leftover, not a stall.
pub(crate) const SCAN_TAIL_LINES: usize = 80;
/// Failed Telegram sends back off this long before either path retries:
/// a dead Telegram must not 429-storm (the watcher wakes every 5s).
pub(crate) const SEND_FAIL_COOL_SECS: u64 = 60;

/// Fresh tail of a wide limit-scan read (the whole screen when shorter).
pub(crate) fn scan_tail(screen: &[String]) -> &[String] {
    if screen.len() > SCAN_TAIL_LINES {
        &screen[screen.len() - SCAN_TAIL_LINES..]
    } else {
        screen
    }
}

/// Same-kind recent-alert suppress: scroll noise never re-pages, but a
/// provider blip must never hide a later quota stall (different kind).
pub(crate) fn alert_suppressed(
    kind: &str,
    prev: Option<(&str, Instant)>,
    now: Instant,
    remind_secs: u64,
) -> bool {
    matches!(prev, Some((k, t)) if k == kind && now.duration_since(t) < Duration::from_secs(remind_secs))
}

/// Kind-flip damping: a changed kind skips one tick so co-present banners
/// swapping the topmost line never spam; persistence pages next tick.
pub(crate) fn kind_flipped(prev_kind: Option<&str>, kind: &str) -> bool {
    prev_kind.is_some_and(|p| p != kind)
}

/// A recent failed send is still cooling down.
pub(crate) fn send_cooled(last_fail: Option<Instant>, now: Instant) -> bool {
    last_fail.is_some_and(|t| now.duration_since(t) < Duration::from_secs(SEND_FAIL_COOL_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::episode::BuzzEpisode;
    use crate::jobs::notices::detect_limit;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_scan_tail_keeps_fresh_window() {
        assert_eq!(scan_tail(&v(&["a", "b"])).len(), 2);
        let big: Vec<String> = (0..200).map(|i| format!("l{i}")).collect();
        let tail = scan_tail(&big);
        assert_eq!(tail.len(), SCAN_TAIL_LINES);
        assert_eq!(tail[0], "l120");
        assert_eq!(tail[SCAN_TAIL_LINES - 1], "l199");
    }

    #[test]
    fn test_suppress_same_kind_only() {
        let t = Instant::now();
        let recent = t - Duration::from_secs(60);
        assert!(alert_suppressed("rate-limit", Some(("rate-limit", recent)), t, 1800));
        assert!(!alert_suppressed("rate-limit", Some(("provider", recent)), t, 1800));
        assert!(!alert_suppressed(
            "rate-limit",
            Some(("rate-limit", t - Duration::from_secs(1900))),
            t,
            1800
        ));
        assert!(!alert_suppressed("rate-limit", None, t, 1800));
    }

    #[test]
    fn test_flip_damping() {
        assert!(!kind_flipped(None, "rate-limit"));
        assert!(!kind_flipped(Some("rate-limit"), "rate-limit"));
        assert!(kind_flipped(Some("provider"), "rate-limit"));
    }

    #[test]
    fn test_send_cooldown() {
        let t = Instant::now();
        assert!(!send_cooled(None, t));
        assert!(send_cooled(Some(t - Duration::from_secs(10)), t));
        assert!(!send_cooled(Some(t - Duration::from_secs(61)), t));
    }

    #[test]
    fn test_unfire_retries_without_restarting() {
        // Delivery failure must re-page next tick, not arm a suppress
        // with nothing delivered and not restart the stuck timer.
        let mut ep = BuzzEpisode::new();
        let t = Instant::now();
        let hit = detect_limit(&v(&["Free usage exceeded, subscribe to Go [retrying in 42s]"]))
            .expect("quota must detect");
        assert!(ep.tick(Some(&hit), t).is_some());
        assert!(ep.tick(Some(&hit), t).is_none());
        ep.unfire();
        assert!(ep.tick(Some(&hit), t).is_some());
    }

    #[test]
    fn test_is_fresh_tracks_episode_lifecycle() {
        // The watcher releases the shared claim exactly on this
        // open → fresh transition (confirmed clear), never sooner.
        let mut ep = BuzzEpisode::new();
        assert!(ep.is_fresh());
        let t = Instant::now();
        let hit = detect_limit(&v(&["Free usage exceeded, subscribe to Go"]))
            .expect("quota must detect");
        assert!(ep.tick(Some(&hit), t).is_some());
        assert!(!ep.is_fresh());
        ep.tick(None, t);
        ep.tick(None, t);
        assert!(!ep.is_fresh());
        ep.tick(None, t);
        assert!(ep.is_fresh());
    }

    #[test]
    fn test_typo_quota_detects_but_plain_typo_stays_silent() {
        let hit = detect_limit(&v(&["rfree usage exceded, subscribe to Go"]))
            .expect("typo quota must detect");
        assert_eq!(hit.kind, "rate-limit");
        assert!(detect_limit(&v(&["we exceded expectations on latency"])).is_none());
    }
}
