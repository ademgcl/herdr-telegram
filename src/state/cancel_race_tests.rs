//! No-job cancel race tests (split from `cancel_tests`, 300-line file limit).
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
