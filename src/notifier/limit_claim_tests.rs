//! Tests for the limit-claim drop backstop (split: 300-line file limit).
use super::*;
use crate::state::cancel::isolated_state;

#[tokio::test]
async fn test_claim_suppresses_same_kind_then_releases_on_drop() {
    let (s, _dir) = isolated_state();
    let now = Instant::now();
    // First claim wins.
    let g = try_claim(&s, "w1:p1", "quota", now)
        .await
        .expect("first claim");
    assert!(
        try_claim(&s, "w1:p1", "quota", Instant::now())
            .await
            .is_none()
    );
    // A provider blip never hides later quota (same-kind only).
    let h = try_claim(&s, "w1:p1", "provider", Instant::now())
        .await
        .expect("other kind claims");
    h.keep();
    drop(g);
    // Drop (watchdog-timeout parity) released iff still ours: a
    // same-kind claim retries instead of silencing for 30 min. The
    // provider claim above overwrote the slot, so the dropped quota
    // guard must NOT have removed it.
    assert_eq!(
        s.limit_alert
            .lock()
            .await
            .get("w1:p1")
            .map(|(k, _)| k.clone()),
        Some("provider".to_string())
    );
}

#[tokio::test]
async fn test_drop_releases_own_claim_for_retry() {
    let (s, _dir) = isolated_state();
    let g = try_claim(&s, "w1:p1", "quota", Instant::now())
        .await
        .expect("claim");
    drop(g);
    assert!(!s.limit_alert.lock().await.contains_key("w1:p1"));
}

#[tokio::test]
async fn test_keep_arms_suppress_and_release_frees() {
    let (s, _dir) = isolated_state();
    let g = try_claim(&s, "w1:p1", "quota", Instant::now())
        .await
        .expect("claim");
    g.keep();
    // Delivered: same-kind suppressed (remind window armed).
    assert!(
        try_claim(&s, "w1:p1", "quota", Instant::now())
            .await
            .is_none()
    );
    // Failed send on a fresh kind releases for retry.
    let h = try_claim(&s, "w1:p1", "provider", Instant::now())
        .await
        .expect("second kind claims");
    h.release().await;
    assert!(
        try_claim(&s, "w1:p1", "provider", Instant::now())
            .await
            .is_some()
    );
}
