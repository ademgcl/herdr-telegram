//! Boot-recovery tests. Split from `recover` (300-line file limit).
use super::*;
use crate::jobs::recover_gate::STALE_KEEP_MAX_SECS;

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

#[tokio::test]
async fn test_recover_clear_spares_resubmit_from_report_window() {
    // report() awaits up to 90s between the pre-check and the clear: a
    // user resubmitting in that window owns the slot — recover's clear
    // must wipe only the exact boot snapshot, never the fresh intent
    // (unconditional clear_pending lost the reply's durable intent).
    use crate::jobs::persist::PendingPrompt;
    let (s, _dir) = crate::state::cancel::isolated_state();
    let boot = PendingPrompt {
        chat: 1,
        thread: None,
        prompt: "hi".into(),
        started_unix: 100,
    };
    s.pending.lock().await.insert("w1:p1".into(), boot.clone());
    // Pre-report ownership check passes for the boot copy…
    assert!(s.pending_matches("w1:p1", 1, None, "hi").await);
    // …then the user resubmits mid-report (fresh stamp, same text).
    let fresh = PendingPrompt {
        started_unix: 200,
        ..boot.clone()
    };
    s.pending.lock().await.insert("w1:p1".into(), fresh.clone());
    // Delivered verdict + superseded slot: keep the fresh intent.
    assert!(!recover_clear(&s, "w1:p1", &boot, true).await);
    assert!(
        s.pending
            .lock()
            .await
            .get("w1:p1")
            .is_some_and(|p| p == &fresh)
    );
    // Undelivered verdict never clears (bounded separately by the 7d cap).
    assert!(!recover_clear(&s, "w1:p1", &fresh, false).await);
    assert!(s.pending.lock().await.contains_key("w1:p1"));
    // Unchanged snapshot + delivered verdict still clears (stale/gone parity).
    assert!(recover_clear(&s, "w1:p1", &fresh, true).await);
    assert!(!s.pending.lock().await.contains_key("w1:p1"));
}
