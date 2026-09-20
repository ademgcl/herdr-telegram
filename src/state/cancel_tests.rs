//! Tests for cancel retire paths (split: 300-line file limit).
use super::*;
use crate::jobs::job::Job;
use crate::jobs::persist::PendingPrompt;

fn prompt(chat: i64) -> PendingPrompt {
    PendingPrompt {
        chat,
        thread: None,
        prompt: "hi".into(),
        started_unix: 0,
    } // fmt:keep 1-line (300-line file limit)
}

#[tokio::test]
async fn test_cancel_retires_job_and_bumps_epoch() {
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    s.pending.lock().await.insert("t:p1".into(), prompt(1));
    assert!(s.cancel_jobs_for("t:p1").await);
    assert!(job.is_stopped());
    assert_eq!(job.epoch.load(Ordering::Relaxed), 1);
    assert!(!s.jobs.lock().await.contains_key("t:p1"));
    assert!(!s.pending.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_remove_if_same_is_last_writer_wins() {
    let (s, _dir) = isolated_state();
    let old = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), old.clone());
    assert!(State::remove_if_same(&s.jobs, "t:p1", &old).await);
    // Successor inserted after the snapshot survives.
    let a = Job::new(vec![], 1, None);
    let b = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), a.clone());
    s.jobs.lock().await.insert("t:p1".into(), b.clone());
    assert!(!State::remove_if_same(&s.jobs, "t:p1", &a).await);
    assert!(Arc::ptr_eq(
        &s.jobs.lock().await.get("t:p1").unwrap().clone(),
        &b
    ));
}

#[tokio::test]
async fn test_quiet_retires_silently() {
    // Quiet retire removes the job like loud but without the cancel
    // notify (the parked watcher exits silently via is_stopped).
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    assert!(s.cancel_jobs_for_quiet("t:p1").await);
    assert!(job.is_stopped());
    assert_eq!(job.epoch.load(Ordering::Relaxed), 1);
    assert!(!s.jobs.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_quiet_clears_limit_episode_loud_parity() {
    // Same-name remint must not inherit stall state after a quiet
    // dead/shell retire (loud parity: cancel_jobs_for clears it).
    let (s, _dir) = isolated_state();
    let now = std::time::Instant::now();
    s.limit_alert
        .lock()
        .await
        .insert("t:p1".into(), ("m".into(), now));
    s.limit_seen
        .lock()
        .await
        .insert("t:p1".into(), ("m".into(), now));
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    assert!(s.cancel_jobs_for_quiet("t:p1").await);
    assert!(!s.limit_alert.lock().await.contains_key("t:p1"));
    assert!(!s.limit_seen.lock().await.contains_key("t:p1"));
    // No-job branch clears too (loud parity).
    s.limit_miss.lock().await.insert("t:p9".into(), 3);
    s.cancel_jobs_for_quiet("t:p9").await;
    assert!(!s.limit_miss.lock().await.contains_key("t:p9"));
}

#[tokio::test]
async fn test_job_only_preserves_pending_intent() {
    // Already-shell branch: stale watcher dies, live shell intent stays.
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    s.pending.lock().await.insert("t:p1".into(), prompt(1));
    assert!(s.cancel_job_only_for("t:p1").await);
    assert!(job.is_stopped());
    assert!(s.pending.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_clear_pending_if_epoch_matches_is_exact() {
    // Match-guarded clear: only the exact submit triple + generation
    // clears — a resubmitted successor survives the loser's clear, a
    // vacant slot clears nothing. An identical re-prompt ("continue"×2)
    // bumps the epoch without changing the text, so text equality alone
    // must not wipe it.
    use std::sync::atomic::AtomicU64;
    let (s, _dir) = isolated_state();
    let epoch = AtomicU64::new(0);
    s.remember_pending("t:p1", 1, None, "make").await;
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", 1, None, "other", 0, &epoch)
            .await
    );
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", 2, None, "make", 0, &epoch)
            .await
    );
    assert!(s.pending.lock().await.contains_key("t:p1"));
    // Stale generation (identical text, bumped epoch): keep.
    epoch.store(1, std::sync::atomic::Ordering::Relaxed);
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", 1, None, "make", 0, &epoch)
            .await
    );
    assert!(s.pending.lock().await.contains_key("t:p1"));
    assert!(
        s.clear_pending_if_epoch_matches("t:p1", 1, None, "make", 1, &epoch)
            .await
    );
    assert!(!s.pending.lock().await.contains_key("t:p1"));
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", 1, None, "make", 1, &epoch)
            .await
    );
}

#[tokio::test]
async fn test_job_only_vacant_keeps_live_debounce() {
    // Vacant-pane retire must not eat a live pane's fresh settle debounce.
    let (s, _dir) = isolated_state();
    let live = Job::new(vec![], 9, None);
    s.jobs.lock().await.insert("t:p9".into(), live);
    s.debounce
        .lock()
        .await
        .insert("t:p9".into(), ("idle".into(), std::time::Instant::now()));
    assert!(!s.cancel_job_only_for("t:vacant").await);
    assert!(s.debounce.lock().await.contains_key("t:p9"));
    // Vacant pane's own stale debounce still clears.
    s.debounce.lock().await.insert(
        "t:vacant".into(),
        ("idle".into(), std::time::Instant::now()),
    );
    assert!(!s.cancel_job_only_for("t:vacant").await);
    assert!(!s.debounce.lock().await.contains_key("t:vacant"));
}

#[tokio::test]
async fn test_job_only_lost_race_keeps_successor_debounce() {
    // Lost-race guard (loud/quiet parity): a successor inserted after
    // the vacant snapshot owns the fresh debounce — the retire leaves
    // it alone. Raced both orders: whichever wins, a live successor
    // never loses its debounce (the pre-guard code ate it, dropping
    // the next settle's card).
    let (s, _dir) = isolated_state();
    for _ in 0..100 {
        s.jobs.lock().await.remove("t:race");
        s.debounce.lock().await.remove("t:race");
        let s2 = s.clone();
        let racer = tokio::spawn(async move {
            let job = Job::new(vec![], 1, None);
            s2.jobs.lock().await.insert("t:race".into(), job);
            s2.debounce
                .lock()
                .await
                .insert("t:race".into(), ("idle".into(), std::time::Instant::now()));
        });
        let _ = s.cancel_job_only_for("t:race").await;
        racer.await.unwrap();
        if s.jobs.lock().await.contains_key("t:race") {
            assert!(s.debounce.lock().await.contains_key("t:race"));
        }
    }
}

#[tokio::test]
async fn test_cancel_all_counts_and_stops() {
    let (s, _dir) = isolated_state();
    let a = Job::new(vec![], 1, None);
    let b = Job::new(vec![], 2, None);
    s.jobs.lock().await.insert("t:p1".into(), a.clone());
    s.jobs.lock().await.insert("t:p2".into(), b.clone());
    assert_eq!(s.cancel_all_jobs().await, 2);
    assert!(a.is_stopped() && b.is_stopped());
    assert!(s.jobs.lock().await.is_empty());
}

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
        s.remember_pending_cas("t:p1", (1, None, "hi"), (1, None, "hi"))
            .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].started_unix, 42);
    // Foreign triple never clobbers.
    assert!(
        !s.remember_pending_cas("t:p1", (1, None, "no"), (1, None, "no"))
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
