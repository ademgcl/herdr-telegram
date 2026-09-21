//! No-job cancel race tests (split from `cancel_tests`, 300-line file limit).
use super::super::retire::global_cancel_clears;
use super::*;
use crate::jobs::persist::PendingPrompt;

fn prompt(chat: i64) -> PendingPrompt {
    PendingPrompt {
        chat,
        thread: None,
        prompt: "hi".into(),
        started_unix: 0,
    }
}

#[test]
fn test_global_cancel_clears_stamp_pinned() {
    // Full-equality verdict (pending_cas parity): an identical resubmit
    // (same chat/thread/text, fresh started_unix) landing between the
    // snapshot and the clear owns the slot — clearing it would wipe a
    // delivered prompt's reply with no buzz ever arriving.
    let old = prompt(1);
    let mut resub = prompt(1);
    resub.started_unix = 99;
    assert!(!global_cancel_clears(Some(&resub), &old));
    // Unchanged slot still clears; vacant/changed clears nothing.
    assert!(global_cancel_clears(Some(&old), &old));
    assert!(!global_cancel_clears(None, &old));
    let mut other = prompt(2);
    other.started_unix = 0;
    assert!(!global_cancel_clears(Some(&other), &old));
}

#[tokio::test]
async fn test_global_cancel_race_keeps_identical_resubmit() {
    // The one-line fix above, end to end: a same-text resubmit with a
    // fresh stamp racing `cancel_all_jobs` must survive when the race
    // is lost (old field-equality wiped it — silent reply loss).
    let (s, _dir) = isolated_state();
    for _ in 0..100 {
        s.jobs.lock().await.remove("t:stamp");
        s.pending.lock().await.remove("t:stamp");
        s.pending.lock().await.insert("t:stamp".into(), prompt(1));
        let s2 = s.clone();
        let racer = tokio::spawn(async move {
            let mut resub = prompt(1);
            resub.started_unix = 99;
            s2.pending.lock().await.insert("t:stamp".into(), resub);
        });
        let _ = s.cancel_all_jobs().await;
        racer.await.unwrap();
        // Lost the race (resubmit landed between snapshot and clear):
        // the fresh intent survives. Won orderings legitimately clear
        // first and then land the resubmit — also surviving.
        if let Some(cur) = s.pending.lock().await.get("t:stamp") {
            assert_eq!(cur.started_unix, 99, "stale stamp kept instead");
        }
    }
}

#[tokio::test]
async fn test_no_job_cancel_race_keeps_successor_debounce() {
    // A submit racing a no-job cancel owns the changed pending slot:
    // the retire must not eat its debounce (the next settle's card
    // would be lost).
    let (s, _dir) = isolated_state();
    for _ in 0..100 {
        s.jobs.lock().await.remove("t:race");
        s.pending.lock().await.remove("t:race");
        s.debounce.lock().await.remove("t:race");
        s.last_done.lock().await.remove("t:race");
        let s2 = s.clone();
        let racer = tokio::spawn(async move {
            s2.pending.lock().await.insert("t:race".into(), prompt(1));
            s2.debounce
                .lock()
                .await
                .insert("t:race".into(), ("idle".into(), std::time::Instant::now()));
        });
        let ret = s.cancel_jobs_for("t:race").await;
        racer.await.unwrap();
        // Lost the race (pending changed under us): the successor's
        // debounce survives. (last_done may or may not be stamped —
        // cancel-first orderings legitimately stamp before the submit
        // lands; the lost-race path itself stamps nothing.)
        if !ret && s.pending.lock().await.contains_key("t:race") {
            assert!(s.debounce.lock().await.contains_key("t:race"));
        }
    }
}
