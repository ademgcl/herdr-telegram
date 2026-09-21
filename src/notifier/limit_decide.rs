//! Pure stall-alert decisions shared by the watcher and watchdog paths.
//! Split from `limits` (300-line file limit): predicates here are
//! unit-tested, the async RPC/send orchestration stays in `limits`/`stall`.
use crate::jobs::notices::{
    LimitHit,
    patterns::{CONTEXT, STRONG, WEAK, best_hit, kind_priority, normalize_line},
};
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

/// No-dest skip verdict (pure, tested): with no owner dest and no forum
/// mapping there is nowhere to send — claiming + cooling would burn the
/// shared claim and delay the real stall a full tick. Forum-mode
/// unmapped panes (paced-reset remint in flight) stay silent for retry
/// (stall.rs parity); DM-mode (no forum) still skips here and lets the
/// owner-broadcast arm below decide.
pub(crate) fn no_dest_skip(has_forum: bool, owners_empty: bool) -> bool {
    owners_empty || has_forum
}

/// Tail detection with full-screen context: settled panes match banners
/// in the fresh tail, but WEAK hits may draw error context from anywhere
/// on screen — tail-only context would read a live stall as clean and
/// clear its episode. Strong hits need no context either way. Mirrors
/// `detect_limit` ranking (priority first, freshest line wins).
pub(crate) fn detect_tail_with_context(tail: &[String], full: &[String]) -> Option<LimitHit> {
    let lower_tail: Vec<String> = tail
        .iter()
        .map(|l| normalize_line(&l.to_lowercase()))
        .collect();
    // Context lines get the same normalization: typo-only context
    // (`exceded`) must count exactly like the working path sees it.
    let lower_full: Vec<String> = full
        .iter()
        .map(|l| normalize_line(&l.to_lowercase()))
        .collect();
    let strong_hit = best_hit(&lower_tail, STRONG, true);
    let context = lower_full
        .iter()
        .any(|l| CONTEXT.iter().any(|c| l.contains(c)));
    let weak_hit = if context {
        best_hit(&lower_tail, WEAK, false)
    } else {
        None
    };
    let mut best: Option<(u8, usize, &'static str, bool)> = None;
    for (hit, strong) in [strong_hit, weak_hit].into_iter().zip([true, false]) {
        if let Some((i, kind)) = hit {
            let p = kind_priority(kind, strong);
            let better = match best {
                None => true,
                Some((bp, bi, _, _)) => p < bp || (p == bp && i > bi),
            };
            if better {
                best = Some((p, i, kind, strong));
            }
        }
    }
    best.map(|(_, i, kind, strong)| LimitHit {
        kind,
        excerpt: crate::jobs::notices::detect::clip(&tail[i]),
        strong,
    })
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
        assert!(alert_suppressed(
            "rate-limit",
            Some(("rate-limit", recent)),
            t,
            1800
        ));
        assert!(!alert_suppressed(
            "rate-limit",
            Some(("provider", recent)),
            t,
            1800
        ));
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
    fn test_no_dest_skip_forum_unmapped_stays_silent() {
        // No owner dest + no forum mapping = nowhere to send: skipping
        // preserves the claim and the cool-down for the retry tick. A
        // forum-mode unmapped pane is a paced-reset remint in flight —
        // claiming would burn the shared claim and cool the real stall
        // a full tick (DM-mode with owners falls through to the owner
        // broadcast arm instead).
        assert!(no_dest_skip(false, true));
        assert!(no_dest_skip(true, true));
        assert!(no_dest_skip(true, false));
        assert!(!no_dest_skip(false, false));
    }

    #[test]
    fn test_unfire_retries_without_restarting() {
        // Delivery failure must re-page next tick, not arm a suppress
        // with nothing delivered and not restart the stuck timer.
        // (rate-limit pages only after the stuck gate — transient quota
        // flashes stay silent.)
        let mut ep = BuzzEpisode::new();
        let t = Instant::now();
        let hit = detect_limit(&v(&[
            "Free usage exceeded, subscribe to Go [retrying in 42s]",
        ]))
        .expect("quota must detect");
        assert!(ep.tick(Some(&hit), t).is_none());
        assert!(ep.tick(Some(&hit), t + Duration::from_secs(91)).is_some());
        assert!(ep.tick(Some(&hit), t + Duration::from_secs(91)).is_none());
        ep.unfire();
        assert!(ep.tick(Some(&hit), t + Duration::from_secs(91)).is_some());
    }

    #[test]
    fn test_is_fresh_tracks_episode_lifecycle() {
        // The watcher releases the shared claim exactly on this
        // open → fresh transition (confirmed clear), never sooner.
        let mut ep = BuzzEpisode::new();
        assert!(ep.is_fresh());
        let t = Instant::now();
        let hit =
            detect_limit(&v(&["Free usage exceeded, subscribe to Go"])).expect("quota must detect");
        assert!(ep.tick(Some(&hit), t).is_none());
        assert!(!ep.is_fresh());
        assert!(ep.tick(Some(&hit), t + Duration::from_secs(91)).is_some());
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

    #[test]
    fn test_tail_mirror_matches_full_detect_on_same_input() {
        // The tail ranking must never drift from detect_limit: on
        // identical input both agree on kind, excerpt and strength.
        let screens = [
            v(&["Free usage exceeded, subscribe to Go [retrying in 42s]"]),
            v(&["error: rate limit hit", "backing off"]),
            v(&["hello there", "  Thought · 300ms"]),
            v(&["upstream error on attempt 9"]),
            // Competing banners: stale strong transient above fresh weak
            // quota — priority must beat recency on both paths.
            v(&[
                "⬝⬝⬝ Provider response headers timed out [retrying attempt #1]",
                "error: upstream replied (429) trouble",
            ]),
        ];
        for s in &screens {
            let full = detect_limit(s).map(|h| (h.kind, h.excerpt, h.strong));
            let tail = detect_tail_with_context(s, s).map(|h| (h.kind, h.excerpt, h.strong));
            assert_eq!(full, tail);
        }
    }

    #[test]
    fn test_tail_weak_banner_uses_full_screen_context() {
        // Fresh WEAK banner in the tail with error words only in deep
        // scrollback is a live stall, not a clean screen.
        let mut full = v(&["build failed: 2 tests red"]);
        full.extend(v(&["plain working line"; 100]));
        full.push("upstream replied (429) trouble".to_string());
        let tail = scan_tail(&full);
        let hit = detect_tail_with_context(tail, &full).expect("weak tail must detect");
        assert_eq!(hit.kind, "rate-limit");
        // Same tail without any context anywhere stays silent.
        let lone = v(&["upstream replied (429) trouble"]);
        assert!(detect_tail_with_context(&lone, &lone).is_none());
        // Typo-only context counts after normalization, exactly like the
        // working path — on quota-plausible lines (`usage` gates the
        // `exceded` → `exceed` fix, so bare-typo prose never qualifies).
        let mut typo_full = v(&["usage budget exceded again"]);
        typo_full.extend(v(&["plain working line"; 100]));
        typo_full.push("upstream replied (429) trouble".to_string());
        let typo_tail = scan_tail(&typo_full);
        let typo_hit =
            detect_tail_with_context(typo_tail, &typo_full).expect("typo context must count");
        assert_eq!(typo_hit.kind, "rate-limit");
    }
}
