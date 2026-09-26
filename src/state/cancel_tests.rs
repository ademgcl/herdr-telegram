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
    assert!(s.remove_job_if_epoch("t:p1", &old, 0).await);
    // Successor inserted after the snapshot survives.
    let a = Job::new(vec![], 1, None);
    let b = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), a.clone());
    s.jobs.lock().await.insert("t:p1".into(), b.clone());
    assert!(!s.remove_job_if_epoch("t:p1", &a, 0).await);
    assert!(Arc::ptr_eq(
        &s.jobs.lock().await.get("t:p1").unwrap().clone(),
        &b
    ));
    // Same-Arc reuse bumps the epoch in place: ptr_eq alone would eat
    // the successor's entry — the epoch pins the generation.
    b.epoch.fetch_add(1, Ordering::Relaxed);
    assert!(!s.remove_job_if_epoch("t:p1", &b, 0).await);
    assert!(s.jobs.lock().await.contains_key("t:p1"));
    assert!(s.remove_job_if_epoch("t:p1", &b, 1).await);
    assert!(!s.jobs.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_cancel_no_job_branch_keeps_racing_intent() {
    // A submit racing a no-job cancel owns the changed slot.
    let (s, _dir) = isolated_state();
    s.pending.lock().await.insert("t:p1".into(), prompt(1));
    let seen: Option<PendingPrompt> = None;
    assert!(!s.clear_pending_if_unchanged("t:p1", seen).await);
    assert!(s.pending.lock().await.contains_key("t:p1"));
    let seen = s.pending.lock().await.get("t:p1").cloned();
    assert!(s.clear_pending_if_unchanged("t:p1", seen).await);
    assert!(!s.pending.lock().await.contains_key("t:p1"));
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
async fn test_cancel_stamps_done_to_abort_dm_spontaneous() {
    // Loud cancel stamps last_done: an in-flight DM spontaneous (no arm
    // to disarm, job already gone) aborts via done-after instead of
    // posting past the /cancel. Quiet retires stamp nothing (liveness
    // covers dead panes; shell flips keep moved-on semantics).
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    let before = std::time::Instant::now();
    assert!(s.cancel_jobs_for("t:p1").await);
    let stamped = s.last_done.lock().await.get("t:p1").copied();
    assert!(stamped.map(|t| t >= before).unwrap_or(false));
    // No-job branch stamps too (shell/shell-intent cancel).
    s.pending.lock().await.insert("t:p9".into(), prompt(1));
    assert!(s.cancel_jobs_for("t:p9").await);
    assert!(s.last_done.lock().await.contains_key("t:p9"));
    // Global cancel stamps every owned pane.
    let j2 = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p2".into(), j2.clone());
    s.cancel_all_jobs().await;
    assert!(s.last_done.lock().await.contains_key("t:p2"));
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
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.remember_pending("t:p1", 1, None, "make").await;
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", &job, 1, None, "other", 0)
            .await
    );
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", &job, 2, None, "make", 0)
            .await
    );
    assert!(s.pending.lock().await.contains_key("t:p1"));
    // Stale generation (identical text, bumped epoch): keep.
    job.epoch.store(1, std::sync::atomic::Ordering::Relaxed);
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", &job, 1, None, "make", 0)
            .await
    );
    assert!(s.pending.lock().await.contains_key("t:p1"));
    assert!(
        s.clear_pending_if_epoch_matches("t:p1", &job, 1, None, "make", 1)
            .await
    );
    assert!(!s.pending.lock().await.contains_key("t:p1"));
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", &job, 1, None, "make", 1)
            .await
    );
}

#[tokio::test]
async fn test_clear_pending_if_epoch_matches_refuses_successor_arc() {
    // Atomic jobs ptr_eq inside the clear: books' detached guard used to
    // release the jobs lock before clearing, so a failed-submit retire
    // (map remove, no epoch bump) + same-text re-prompt in that window
    // matched text+epoch and wiped the successor's fresh intent. A
    // mapped DIFFERENT Arc must refuse; vacant and self-owned still clear.
    let (s, _dir) = isolated_state();
    let old = Job::new(vec![], 1, None);
    let successor = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), successor.clone());
    s.remember_pending("t:p1", 1, None, "continue").await;
    // Successor owns the pane: old bookkeeping must NOT clear, even
    // though its own epoch (0) and the text still match.
    assert!(
        !s.clear_pending_if_epoch_matches("t:p1", &old, 1, None, "continue", 0)
            .await
    );
    assert!(s.pending.lock().await.contains_key("t:p1"));
    // Same-arc owner still clears (epoch pinned).
    assert!(
        s.clear_pending_if_epoch_matches("t:p1", &successor, 1, None, "continue", 0)
            .await
    );
    assert!(!s.pending.lock().await.contains_key("t:p1"));
    // Vacant pane (we already retired the map entry) still clears ours.
    s.remember_pending("t:p2", 1, None, "continue").await;
    assert!(
        s.clear_pending_if_epoch_matches("t:p2", &old, 1, None, "continue", 0)
            .await
    );
    assert!(!s.pending.lock().await.contains_key("t:p2"));
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
