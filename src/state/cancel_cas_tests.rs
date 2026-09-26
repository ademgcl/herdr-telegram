//! CAS stamp + job_live tests (split from `cancel_tests`, 300-line limit).
use super::*;
use crate::jobs::job::Job;

#[tokio::test]
async fn test_cas_with_time_keeps_original_stamp() {
    // Failed-notice restore into a vacant slot must keep the ORIGINAL
    // timestamp: a fresh stamp per retry would defeat the 24h stale
    // bound and keep the corpse intent immortal. A racing submit's
    // newer text still wins (CAS refuses).
    let (s, _dir) = isolated_state();
    assert!(
        s.remember_pending_cas_with_time("t:p1", (1, None, "hi"), (1, None, "hi"), Some(42))
            .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].started_unix, 42);
    // Same-triple re-restore without a stamp keeps it too.
    assert!(
        s.remember_pending_cas_with_time("t:p1", (1, None, "hi"), (1, None, "hi"), None)
            .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].started_unix, 42);
    // Foreign triple never clobbers.
    assert!(
        !s.remember_pending_cas_with_time("t:p1", (1, None, "no"), (1, None, "no"), None)
            .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].prompt, "hi");
}

#[tokio::test]
async fn test_cas_occupied_match_keeps_live_stamp() {
    // Corpse-stamp regression: an identical re-prompt racing the
    // close/vanish RPCs holds a fresh stamp — restoring the corpse's
    // older stamp over it would age the fresh intent toward the 24h
    // stale drop. The passed stamp applies to vacant slots only.
    let (s, _dir) = isolated_state();
    s.remember_pending("t:p1", 1, None, "continue").await;
    let fresh = s.pending.lock().await["t:p1"].started_unix;
    assert!(
        s.remember_pending_cas_with_time(
            "t:p1",
            (1, None, "continue"),
            (1, None, "continue"),
            Some(42)
        )
        .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].started_unix, fresh);
}

#[tokio::test]
async fn test_job_live_ignores_stopped_corpse() {
    // A stopped corpse between mark_stopped() and map removal must not
    // read as an active watcher (one-shot settle checks would abort
    // and lose their reply with no retry).
    let (s, _dir) = isolated_state();
    assert!(!s.job_live("t:p1").await);
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    assert!(s.job_live("t:p1").await);
    job.mark_stopped();
    assert!(!s.job_live("t:p1").await);
}
