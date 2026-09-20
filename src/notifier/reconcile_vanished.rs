//! Vanished-agent retire (agent→shell flip with an owed prompt):
//! split from `reconcile` (300-line file limit). Retires the dead
//! watcher and surfaces the shell tail as the reply; a failed notice
//! restores the intent with its ORIGINAL timestamp so the 24h stale
//! bound still fires instead of retrying forever.
use crate::{
    herdr::client::read_shell_output, jobs::finalize::report, jobs::persist::PendingPrompt,
    state::AppState,
};

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
            s.pending_matches(pane, pp.chat, pp.thread, &pp.prompt)
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
    // with no restore.
    if !mine {
        s.cancel_job_only_for(pane).await;
        return;
    }
    // Re-check after the status-claim await above: a submit landing
    // between the first verdict and now owns the pane — quiet would wipe
    // its fresh intent with no restore (tail re-check only covers
    // post-quiet racers). Same job-only retire as a turned-over intent.
    if let Some(pp) = &owed
        && !s
            .pending_matches(pane, pp.chat, pp.thread, &pp.prompt)
            .await
    {
        s.cancel_job_only_for(pane).await;
        return;
    }
    if owed.is_none() && s.pending.lock().await.contains_key(pane) {
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
        if let Some(cur) = cur
            && (cur.chat != pp.chat || cur.thread != pp.thread || cur.prompt != pp.prompt)
        {
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
        // CAS: an observation landing during the RPCs wins — never
        // clobber it.
        let mut st = s.status.lock().await;
        if st.get(pane).map(|v| v == "shell").unwrap_or(false) {
            st.insert(pane.to_string(), "shell".to_string());
        }
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
