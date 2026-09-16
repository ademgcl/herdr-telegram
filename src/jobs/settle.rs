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

/// One settle check for a settled-sampled status: flap-collapse, time
/// confirmation, then `finalize` (with cancellable outage backoff).
/// `settled_since` arms on the first confirmed sample; the working
/// recheck below clears it, and the caller clears it on working
/// samples, herdr errors, and epoch changes.
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
    settled_since: &mut Option<Instant>,
) -> SettleStep {
    // Collapse done↔idle flapping before committing to a report.
    tokio::time::sleep(Duration::from_millis(750)).await;
    if let Ok(a) = get_agent(&s.cfg.socket, pane).await
        && a.status == "working"
    {
        *settled_since = None;
        return SettleStep::Continue;
    }
    // Time-based confirmation: the first settled sample arms the timer,
    // only persistence commits. Sample counting alone retires on two
    // sub-second event wakes inside one transient gap.
    let now = Instant::now();
    match settled_since {
        Some(t) if now.duration_since(*t) >= Duration::from_secs(SETTLED_CONFIRM_SECS) => {
            *settled_since = None;
        }
        Some(_) => return SettleStep::Continue,
        None => {
            *settled_since = Some(now);
            return SettleStep::Continue;
        }
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
            _ = tokio::time::sleep(Duration::from_secs(*retry_wait)) => {}
        }
        *retry_wait = (*retry_wait * 2).min(60);
        return SettleStep::Continue;
    }
    SettleStep::Break
}
