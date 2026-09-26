//! Tests for [`super::enqueue`] (split: 300-line file limit).
use super::*;
use crate::jobs::job::Job;
use crate::state::cancel::isolated_state;

#[tokio::test]
async fn test_retire_if_idle_stops_and_removes_when_nothing_owed() {
    let (s, _dir) = isolated_state();
    let job = Job::new(Vec::new(), 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    assert!(retire_if_idle(&s, "t:p1", &job).await);
    assert!(job.is_stopped());
    assert!(!s.jobs.lock().await.contains_key("t:p1"));
    // Durable slot untouched (racing shell/corpsed intent survives).
    assert!(!s.pending.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_retire_if_idle_refuses_when_cover_present() {
    let (s, _dir) = isolated_state();
    let job = Job::new(Vec::new(), 1, None);
    s.jobs.lock().await.insert("t:p1".into(), job.clone());
    job.publish_submit(1, None, "hi").await;
    assert!(!retire_if_idle(&s, "t:p1", &job).await);
    assert!(!job.is_stopped());
    assert!(s.jobs.lock().await.contains_key("t:p1"));
    assert_eq!(*job.pending.lock().await, 1);
}

#[tokio::test]
async fn test_retire_if_idle_hold_excludes_interleaved_publish() {
    // The hold must span stop+remove: a concurrent publish_submit waits
    // on pending, so it can only land after the job is already stopped
    // and unmapped — success rearm sees vacant/stopped, never live self
    // (rearm_verdict false ⇒ delivered prompt strands watcherless).
    let (s, _dir) = isolated_state();
    for _ in 0..50 {
        let job = Job::new(Vec::new(), 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job.clone());
        let j2 = job.clone();
        let racer = tokio::spawn(async move {
            j2.publish_submit(1, None, "raced").await;
        });
        let retired = retire_if_idle(&s, "t:p1", &job).await;
        racer.await.unwrap();
        if retired {
            assert!(job.is_stopped());
            assert!(
                !s.jobs.lock().await.contains_key("t:p1"),
                "retire must remove before releasing pending"
            );
        } else {
            assert_eq!(*job.pending.lock().await, 1);
            assert!(!job.is_stopped());
            assert!(s.jobs.lock().await.contains_key("t:p1"));
        }
    }
}
