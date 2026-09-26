//! Vanished-agent retire (agent→shell flip with an owed prompt):
//! split from `reconcile` (300-line file limit). Retires the dead
//! watcher and surfaces the shell tail as the reply; a failed notice
//! restores the intent with its ORIGINAL timestamp so the 24h stale
//! bound still fires instead of retrying forever.
use crate::{
    herdr::client::read_shell_output, jobs::finalize::report, jobs::persist::PendingPrompt,
    state::AppState,
};

/// Undo the report-slot claim when the pane turned over mid-flight: the
/// `shell` stamp belongs to the dead turn, never the live successor.
async fn restore_status(s: &AppState, pane: &str, prev: Option<String>) {
    let mut st = s.status.lock().await;
    if st.get(pane).map(|v| v == "shell").unwrap_or(false) {
        match prev {
            Some(p) => {
                st.insert(pane.to_string(), p);
            }
            None => {
                st.remove(pane);
            }
        }
    }
}

/// Retire the watcher of a vanished agent and report the shell tail.
/// Consumes `owed` (the pre-flip intent, if any). Never returns early
/// with the intent dropped: success clears it via the quiet retire
/// above, failure restores it (CAS + original stamp) for the next tick.
pub async fn retire_vanished(s: &AppState, pane: &str, owed: Option<PendingPrompt>) {
    // Race verdict BEFORE retiring: quiet clears the slot, so a
    // post-quiet check would always read false and drop the notice. A
    // racer-retired intent has its own ack. (Micro-race: submit between
    // check and retire — microseconds, no RPC between.)
    let mine = match &owed {
        Some(pp) => {
            // Stamp-pinned: an identical re-prompt racing the flip owns
            // the slot now (same triple, fresh stamp) — the stale quit
            // card must not misattribute to the new turn.
            s.pending_matches_stamp(pane, pp.chat, pp.thread, &pp.prompt, pp.started_unix)
                .await
        }
        // Job-only flip: no competing intent.
        None => true,
    };
    // Claim the report slot first: a concurrent tick (watchdog + event
    // reconnect) must not double-report the same quit — the loser sees
    // shell and stands down.
    let prev_status = {
        s.status
            .lock()
            .await
            .insert(pane.to_string(), "shell".to_string())
    };
    if prev_status.as_deref() == Some("shell") {
        return;
    }
    // Turned-over intent (submit raced the flip): retire the dead
    // watcher only, preserving the new pending — quiet would wipe it
    // with no restore. Restore the pre-claim status first: the `shell`
    // stamp above would else cover the new turn, and the limit scanner
    // skips `shell` while spontaneous suppresses it as moved-on.
    if !mine {
        restore_status(s, pane, prev_status).await;
        s.cancel_job_only_for(pane).await;
        return;
    }
    // Re-check after the status-claim await above: a submit landing
    // between the first verdict and now owns the pane — quiet would wipe
    // its fresh intent with no restore (tail re-check only covers
    // post-quiet racers). Same job-only retire as a turned-over intent.
    if let Some(pp) = &owed
        && !s
            .pending_matches_stamp(pane, pp.chat, pp.thread, &pp.prompt, pp.started_unix)
            .await
    {
        restore_status(s, pane, prev_status).await;
        s.cancel_job_only_for(pane).await;
        return;
    }
    if owed.is_none() && s.pending.lock().await.contains_key(pane) {
        restore_status(s, pane, prev_status).await;
        s.cancel_job_only_for(pane).await;
        return;
    }
    s.cancel_jobs_for_quiet(pane).await;
    let Some(pp) = owed else {
        s.status
            .lock()
            .await
            .insert(pane.to_string(), "shell".to_string());
        return;
    };
    let tail = read_shell_output(&s.cfg.socket, pane, 60)
        .await
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    // Re-validate after the tail RPC: a submit racing the read owns the
    // pane now — its own card path serves the reply, and a stale quit
    // card beside it misattributes the turn. Vacant still means ours
    // (quiet cleared our slot above).
    {
        let cur = s.pending.lock().await.get(pane).cloned();
        // Stamp-pinned like the gates above: an identical re-prompt
        // landing during the tail read owns the pane now.
        if let Some(cur) = cur
            && (cur.chat != pp.chat
                || cur.thread != pp.thread
                || cur.prompt != pp.prompt
                || cur.started_unix != pp.started_unix)
        {
            // Racer owns the slot — undo our `shell` status claim or the
            // live turn is hidden from the limit scanner / spontaneous
            // suppress forever (restore_status parity with the early arms).
            restore_status(s, pane, prev_status).await;
            return;
        }
    }
    // Always notify (even with an empty tail): the owed prompt retires
    // here, silently dropping it would miss the reply with no retry.
    let msg = if tail.is_empty() {
        "agent quit to shell.".to_string()
    } else {
        format!("agent quit to shell — last output:\n{tail}")
    };
    if report(s, pp.chat, pp.thread, pane, &msg).await {
        return;
    }
    // Intent was already cancelled above: keep it so boot-recover
    // retries the notice instead of losing it. CAS restore so the next
    // tick retries instead of going quiet.
    // Guarded like the dead-close restore: a submit racing the RPCs
    // wins (atomic check-and-set: a check-then-remember across awaits
    // would overwrite it). Original timestamp, never now: a fresh stamp
    // per failed tick would defeat the 24h stale bound and retry forever.
    let restored = s
        .remember_pending_cas_with_time(
            pane,
            (pp.chat, pp.thread, &pp.prompt),
            (pp.chat, pp.thread, &pp.prompt),
            Some(pp.started_unix),
        )
        .await;
    // Poison guard: a submit racing the report RPCs owns the pane now —
    // restoring the agent status would re-arm RetireVanished on the next
    // tick, which eats the fresh shell intent via cancel_jobs_for_quiet
    // and misattributes a stale quit card to the new command. Restore
    // the flip marker only when our owed intent still owns the slot (or
    // it is vacant — the CAS above just re-armed it); a raced submit
    // keeps "shell" so the next tick ignores/preserves its live work
    // (tail-race precedent above: new work wins, old notice drops).
    if restored {
        let mut st = s.status.lock().await;
        if st.get(pane).map(|v| v == "shell").unwrap_or(false) {
            match prev_status {
                Some(p) => {
                    st.insert(pane.to_string(), p);
                }
                None => {
                    st.remove(pane);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::persist::PendingPrompt;
    use crate::state::cancel::isolated_state;

    fn pp(prompt: &str, stamp: u64) -> PendingPrompt {
        PendingPrompt {
            chat: 1,
            thread: None,
            prompt: prompt.into(),
            started_unix: stamp,
        }
    }

    #[tokio::test]
    async fn test_restore_status_undoes_shell_claim_only() {
        // The report-slot claim belongs to the dead turn: restore puts
        // the prior value back ONLY while the slot still reads "shell"
        // (a live successor's status must never be clobbered).
        let (s, _dir) = isolated_state();
        // Vacant: nothing to undo.
        restore_status(&s, "t:p1", Some("working".into())).await;
        assert!(!s.status.lock().await.contains_key("t:p1"));
        // Claimed shell + prior value → restored.
        s.status.lock().await.insert("t:p1".into(), "shell".into());
        restore_status(&s, "t:p1", Some("working".into())).await;
        assert_eq!(s.status.lock().await["t:p1"], "working");
        // Claimed shell + no prior → removed (boot-empty parity).
        s.status.lock().await.insert("t:p1".into(), "shell".into());
        restore_status(&s, "t:p1", None).await;
        assert!(!s.status.lock().await.contains_key("t:p1"));
        // Successor flipped the slot off shell: leave it alone.
        s.status
            .lock()
            .await
            .insert("t:p1".into(), "working".into());
        restore_status(&s, "t:p1", Some("idle".into())).await;
        assert_eq!(s.status.lock().await["t:p1"], "working");
    }

    #[tokio::test]
    async fn test_retire_vanished_turned_over_restores_status_keeps_successor() {
        // Racer-owned slot (stamp mismatch): the shell claim must be
        // undone (or the live turn hides from limit scan / spontaneous
        // forever) and the successor's pending + waiter job survive.
        let (s, _dir) = isolated_state();
        s.status
            .lock()
            .await
            .insert("t:p1".into(), "working".into());
        s.pending
            .lock()
            .await
            .insert("t:p1".into(), pp("fresh", 99));
        let job = crate::jobs::job::Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job.clone());
        retire_vanished(&s, "t:p1", Some(pp("corpse", 42))).await;
        assert_eq!(
            s.status.lock().await["t:p1"],
            "working",
            "shell claim must be restored for the successor"
        );
        assert_eq!(s.pending.lock().await["t:p1"].prompt, "fresh");
        assert_eq!(s.pending.lock().await["t:p1"].started_unix, 99);
        assert!(
            !s.jobs.lock().await.contains_key("t:p1"),
            "stale corpse watcher still retires job-only"
        );
    }

    #[tokio::test]
    async fn test_retire_vanished_late_pending_restores_status() {
        // owed=None (job-only flip) with a submit landing mid-flight:
        // job-only retire, status restored — the fresh intent serves its
        // own reply, never hidden behind the shell claim.
        let (s, _dir) = isolated_state();
        s.status
            .lock()
            .await
            .insert("t:p1".into(), "working".into());
        let job = crate::jobs::job::Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job.clone());
        s.pending.lock().await.insert("t:p1".into(), pp("late", 7));
        retire_vanished(&s, "t:p1", None).await;
        assert_eq!(s.status.lock().await["t:p1"], "working");
        assert_eq!(s.pending.lock().await["t:p1"].prompt, "late");
        assert!(!s.jobs.lock().await.contains_key("t:p1"));
    }

    #[tokio::test]
    async fn test_retire_vanished_double_claim_stands_down() {
        // Concurrent tick already claimed the report slot ("shell"):
        // the loser returns before any retire — never double-report.
        let (s, _dir) = isolated_state();
        s.status.lock().await.insert("t:p1".into(), "shell".into());
        let job = crate::jobs::job::Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job.clone());
        retire_vanished(&s, "t:p1", None).await;
        assert_eq!(s.status.lock().await["t:p1"], "shell");
        assert!(s.jobs.lock().await.contains_key("t:p1"));
    }
}
