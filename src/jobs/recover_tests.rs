//! Boot-recovery tests. Split from `recover` (300-line file limit).
use super::*;

#[test]
fn test_recoverable_windows() {
    let now = 1_800_000_000;
    assert!(recoverable(now - 10, now)); // fresh
    assert!(recoverable(now - 86400, now)); // boundary
    assert!(!recoverable(now - 86401, now)); // stale
    assert!(!recoverable(now + 3601, now)); // future clock jump
    assert!(recoverable(now + 60, now)); // small skew tolerated
}

#[test]
fn test_claim_watcher_single_owner_wins() {
    // First claim inserts; a racing second claim for the same pane
    // stands down (no duplicate watchers → no double alerts).
    let mut map = std::collections::HashMap::new();
    let a = Job::new(vec![], 1, None);
    let b = Job::new(vec![], 2, None);
    assert!(claim_watcher(&mut map, "w1:p1", a.clone()));
    assert!(!claim_watcher(&mut map, "w1:p1", b.clone()));
    assert!(std::sync::Arc::ptr_eq(map.get("w1:p1").unwrap(), &a));
    // Distinct pane still claims.
    assert!(claim_watcher(&mut map, "w1:p2", b));
}

#[test]
fn test_claim_watcher_stopped_corpse_does_not_block() {
    // A stopped corpse left in the map must not leave durable intent
    // watcherless: the re-arm replaces it.
    let mut map = std::collections::HashMap::new();
    let corpse = Job::new(vec![], 1, None);
    corpse.mark_stopped();
    map.insert("w1:p9".to_string(), corpse);
    let fresh = Job::new(vec![], 2, None);
    assert!(claim_watcher(&mut map, "w1:p9", fresh.clone()));
    assert!(std::sync::Arc::ptr_eq(map.get("w1:p9").unwrap(), &fresh));
}

#[test]
fn test_stale_drop_should_clear_bounded() {
    let now = 1_800_000_000;
    let stale = now - 90000; // >24h, within 7d keep window
    // Delivered notices always clear.
    assert!(stale_drop_should_clear(true, stale, now));
    // Undelivered notices keep the intent for the next boot…
    assert!(!stale_drop_should_clear(false, stale, now));
    // …but a 7d+ corpse clears even when Telegram is dead (no
    // endless retry), both directions of the clock.
    assert!(stale_drop_should_clear(
        false,
        now - STALE_KEEP_MAX_SECS - 1,
        now
    ));
    assert!(stale_drop_should_clear(
        false,
        now + STALE_KEEP_MAX_SECS + 1,
        now
    ));
    // Inside the keep window, future-jump corpses still wait.
    assert!(!stale_drop_should_clear(false, now + 7200, now));
}
