//! Queue-clear coverage for cancel retire paths (split: 300-line
//! file limit). A stranded prompt queue (non-empty, no live turn) must
//! die on every cancel path — otherwise /cancel can't recover it and a
//! later turn resurrects a prompt the user thought was cancelled.
use super::*;
use crate::jobs::job::Job;
use crate::jobs::queue::{QueuedPrompt, clear_queue, push_queue, queue_len};

fn held(chat: i64) -> QueuedPrompt {
    QueuedPrompt {
        chat_id: chat,
        thread_id: None,
        text: "queued behind a dead turn".into(),
    } // fmt:keep 1-line (300-line file limit)
}

#[tokio::test]
async fn test_loud_no_job_clears_stranded_queue() {
    let (s, _dir) = isolated_state();
    push_queue(&s, "t:p1", held(1)).await.unwrap();
    // No job mapped: /cancel reports nothing running but must still
    // drain the stranded queue (the only recovery when no watcher serves).
    s.cancel_jobs_for("t:p1").await;
    assert_eq!(queue_len(&s, "t:p1").await, 0);
}

#[tokio::test]
async fn test_quiet_no_job_clears_stranded_queue() {
    let (s, _dir) = isolated_state();
    push_queue(&s, "t:p1", held(1)).await.unwrap();
    s.cancel_jobs_for_quiet("t:p1").await;
    assert_eq!(queue_len(&s, "t:p1").await, 0);
}

#[tokio::test]
async fn test_job_only_clears_queue_job_and_no_job() {
    let (s, _dir) = isolated_state();
    // Job branch: stale watcher retired, queue must not survive it.
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    push_queue(&s, "t:p1", held(1)).await.unwrap();
    assert!(s.cancel_job_only_for("t:p1").await);
    assert_eq!(queue_len(&s, "t:p1").await, 0);
    // No-job branch: stranded queue with no watcher dies too.
    push_queue(&s, "t:p2", held(1)).await.unwrap();
    assert!(!s.cancel_job_only_for("t:p2").await);
    assert_eq!(queue_len(&s, "t:p2").await, 0);
}

#[tokio::test]
async fn test_loud_job_branch_still_clears_queue() {
    // Regression pin: the pre-existing job-branch clear keeps working.
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    push_queue(&s, "t:p1", held(1)).await.unwrap();
    assert!(s.cancel_jobs_for("t:p1").await);
    assert_eq!(queue_len(&s, "t:p1").await, 0);
    // clear_queue stays idempotent for the quiet path below.
    clear_queue(&s, "t:p1").await;
}
