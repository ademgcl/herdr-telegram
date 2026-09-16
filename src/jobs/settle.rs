//! Settle confirmation + finalization for prompt watchers. Split from
//! `runner` (300-line file limit): one settled sample must not retire the
//! watcher — agy idles briefly between phases mid-run, and retiring on
//! that transient leaves the agent working unwatched (no final card at
//! true completion) while the watchdog spams stall cards off prose.
use crate::{
    herdr::client::get_agent,
    jobs::finalize::{edit_live, finalize},
    jobs::job::Job,
    state::AppState,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::time::{Duration, Instant};

/// Minimum seconds a settled status must persist before the report
/// commits. Event-driven wakes can land sub-second apart, so counting
/// samples alone still retires on one short transient gap — the first
/// settled sample only arms the timer, persistence commits it. Genuine
/// settles arrive this much later; transients never do.
const SETTLED_CONFIRM_SECS: u64 = 5;

/// Watcher verdict after one settle check.
pub enum SettleStep {
    Continue,
    Break,
}

/// Pure recheck rule: the armed sample's persistence claim holds only
/// while the status still reads the SAME settled kind. (The timer below
/// tracks the arming kind, so this documents the rule where the 750ms
/// recheck applies it.)
fn confirm_still_valid(sampled: &str, rechecked: &str) -> bool {
    rechecked == sampled
}

/// Armed settle timer: when the first settled sample landed + which
/// kind armed it. Persistence must be same-kind: a settled-kind flip
/// (done→blocked→idle) re-arms on the new kind instead of committing
/// the flap as persistence.
pub type SettledArm = Option<(Instant, String)>;

/// Pure report-commit decision for the settle timer: the first settled
/// sample arms it (recording its kind); only same-kind persistence past
/// [`SETTLED_CONFIRM_SECS`] commits. A settled-kind flip re-arms on the
/// new kind. Unit-tested — the async wrapper only feeds it samples.
fn confirm_due(armed: &mut SettledArm, status: &str, now: Instant) -> bool {
    match armed {
        Some((t, kind)) if kind == status => {
            if now.duration_since(*t) >= Duration::from_secs(SETTLED_CONFIRM_SECS) {
                *armed = None;
                true
            } else {
                false
            }
        }
        _ => {
            *armed = Some((now, status.to_string()));
            false
        }
    }
}

/// One settle check for a settled-sampled status: flap-collapse, time
/// confirmation, then `finalize` (with cancellable outage backoff).
/// `settled_since` arms on the first confirmed sample (recording its
/// kind); the working recheck below clears it, kind flips re-arm it,
/// and the caller clears it on working samples, herdr errors, and
/// epoch changes.
#[allow(clippy::too_many_arguments)]
pub async fn settle_step(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    status: &str,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    acc: &mut Vec<String>,
    retry_wait: &mut u64,
    settled_since: &mut SettledArm,
) -> SettleStep {
    // Collapse done↔idle flapping before committing to a report. A
    // failed recheck is unknown, not settled: clear the timer (the
    // caller's herdr-error arm does the same) so a post-outage sample
    // never counts as persistence spanning the blackout. A settled-kind
    // flip (done→blocked→idle) also clears: persistence of one kind is
    // not persistence of another.
    tokio::time::sleep(Duration::from_millis(750)).await;
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status == "working" => {
            *settled_since = None;
            return SettleStep::Continue;
        }
        Ok(a) if !confirm_still_valid(status, &a.status) => {
            *settled_since = None;
            return SettleStep::Continue;
        }
        Err(e) => {
            eprintln!("[watcher] {pane} confirming read failed: {e}");
            *settled_since = None;
            return SettleStep::Continue;
        }
        _ => {}
    }
    // Time-based confirmation: the first settled sample arms the timer
    // (recording its kind), only same-kind persistence commits. Sample
    // counting alone retires on two sub-second event wakes inside one
    // transient gap.
    if !confirm_due(settled_since, status, Instant::now()) {
        return SettleStep::Continue;
    }
    let epoch_before = job.epoch.load(Ordering::Relaxed);
    let retry = finalize(s, pane, job, status, live_mid, acc).await;
    // finalize consumes the live slot on success — drop its
    // address too, or a later reset would edit the final card.
    if live_mid.is_none() {
        *live_dest = None;
    }
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return SettleStep::Continue;
    }
    if retry {
        // Delivery/read outage: back off (capped) instead of
        // retiring — the intent stays until /cancel or pane death.
        // Cancellable like the unreachable backoff in the runner.
        // Supersede signals via epoch bump only (never notify), so the
        // backoff watches it too — else a new prompt landing mid-backoff
        // stalls its handoff for up to a minute.
        let epoch_now = job.epoch.load(Ordering::Relaxed);
        tokio::select! {
            _ = job.cancel.notified() => {
                job.mark_stopped();
                // Like the sibling cancel branches: a genuine
                // cancel retires the durable intent (a supersede
                // never notifies — it bumps the epoch instead).
                if s.jobs.lock().await.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
                    s.clear_pending(pane).await;
                }
                let (chat, th) = *job.dest.lock().await;
                edit_live(s, chat, th, pane, live_mid, "✋ cancelled").await;
                return SettleStep::Break;
            }
            _ = sleep_or_superseded(job, epoch_now, Duration::from_secs(*retry_wait)) => {}
        }
        *retry_wait = (*retry_wait * 2).min(60);
        return SettleStep::Continue;
    }
    SettleStep::Break
}

/// Sleep up to `dur`, returning early when the job's epoch moves
/// (a superseding prompt took over mid-backoff).
async fn sleep_or_superseded(job: &Arc<Job>, epoch: u64, dur: Duration) {
    let end = Instant::now() + dur;
    while job.epoch.load(Ordering::Relaxed) == epoch {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        tokio::time::sleep(left.min(Duration::from_millis(250))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirm_arms_then_fires_on_persistence() {
        let t0 = Instant::now();
        let mut since: SettledArm = None;
        // First settled sample only arms (recording its kind).
        assert!(!confirm_due(&mut since, "done", t0));
        assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("done"));
        // Sub-second event wakes inside one transient never commit.
        assert!(!confirm_due(&mut since, "done", t0 + Duration::from_millis(800)));
        assert!(!confirm_due(&mut since, "done", t0 + Duration::from_secs(4)));
        // Same-kind persistence past the gate commits and disarms.
        assert!(confirm_due(&mut since, "done", t0 + Duration::from_secs(5)));
        assert!(since.is_none());
    }

    #[test]
    fn test_confirm_rearms_on_kind_flip() {
        let t0 = Instant::now();
        let mut since: SettledArm = None;
        assert!(!confirm_due(&mut since, "done", t0));
        // done→blocked→idle flips re-arm instead of committing.
        assert!(!confirm_due(&mut since, "blocked", t0 + Duration::from_secs(4)));
        assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("blocked"));
        assert!(!confirm_due(&mut since, "idle", t0 + Duration::from_secs(8)));
        assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("idle"));
        // Only 5s of the SAME kind commits.
        assert!(!confirm_due(&mut since, "idle", t0 + Duration::from_secs(12)));
        assert!(confirm_due(&mut since, "idle", t0 + Duration::from_secs(13)));
        assert!(since.is_none());
    }

    #[test]
    fn test_recheck_clears_timer_on_kind_flip() {
        // Same settled kind: persistence claim holds.
        assert!(confirm_still_valid("done", "done"));
        assert!(confirm_still_valid("idle", "idle"));
        assert!(confirm_still_valid("blocked", "blocked"));
        // Settled-kind flips void it (done→blocked→idle must not commit
        // as one persistence); working always clears.
        assert!(!confirm_still_valid("done", "blocked"));
        assert!(!confirm_still_valid("blocked", "idle"));
        assert!(!confirm_still_valid("done", "idle"));
        assert!(!confirm_still_valid("idle", "done"));
        assert!(!confirm_still_valid("done", "working"));
    }
    #[test]
    fn test_confirm_is_event_rate_independent() {
        // Ten rapid wakes inside a 2s transient: still no commit.
        let t0 = Instant::now();
        let mut since: SettledArm = None;
        for ms in (0..2000).step_by(200) {
            assert!(
                !confirm_due(&mut since, "idle", t0 + Duration::from_millis(ms)),
                "must not fire at {ms}ms"
            );
        }
    }
}
