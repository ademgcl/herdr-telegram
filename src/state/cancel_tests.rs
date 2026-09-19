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
async fn test_clear_pending_if_matches_is_exact() {
    // Match-guarded clear: only the exact submit triple clears — a
    // resubmitted successor survives the loser's clear, a vacant slot
    // clears nothing.
    let (s, _dir) = isolated_state();
    s.remember_pending("t:p1", 1, None, "make").await;
    assert!(!s.clear_pending_if_matches("t:p1", 1, None, "other").await);
    assert!(!s.clear_pending_if_matches("t:p1", 2, None, "make").await);
    assert!(s.pending.lock().await.contains_key("t:p1"));
    assert!(s.clear_pending_if_matches("t:p1", 1, None, "make").await);
    assert!(!s.pending.lock().await.contains_key("t:p1"));
    assert!(!s.clear_pending_if_matches("t:p1", 1, None, "make").await);
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
