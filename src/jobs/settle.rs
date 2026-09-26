//! Settle confirmation + finalization for prompt watchers. Split from
//! `runner` (300-line file limit): one settled sample must not retire the
//! watcher — agy idles briefly between phases mid-run, and retiring on
//! that transient leaves the agent working unwatched (no final card at
//! true completion) while the watchdog spams stall cards off prose.
use crate::{
    herdr::client::get_agent, jobs::finalize::finalize, jobs::job::Job,
    jobs::repoint::repoint_dest_if_remapped, state::AppState,
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

/// Commit rule for one settle sample: `blocked` commits at once (input
/// is needed NOW; blocked is never a mid-run transient, and the 750ms
/// recheck upstream already filtered blips), and so does a stuck fatal
/// provider error (the run is over — waiting out 5s of same-kind
/// persistence while the status flaps around the error would loop the
/// watcher forever on a frozen "working" card; finalize arbitration
/// still picks the error screen, so nothing healthy is cut short).
/// Other settled kinds still prove [`SETTLED_CONFIRM_SECS`] persistence.
/// Never arms the timer for instant commits, so no stale arm survives them.
fn settle_commit(status: &str, armed: &mut SettledArm, now: Instant, fatal_stuck: bool) -> bool {
    if fatal_stuck || status == "blocked" {
        *armed = None;
        return true;
    }
    confirm_due(armed, status, now)
}

/// One settle check for a settled-sampled status: flap-collapse, time
/// confirmation, then `finalize` (with cancellable outage backoff).
/// `settled_since` arms on the first confirmed sample (recording its
/// kind); the working recheck below clears it, kind flips re-arm it,
/// and the caller clears it on working samples, herdr errors, and
/// epoch changes. `loop_epoch` is the runner's generation at loop top:
/// an entry epoch that differs means a submit landed between the
/// runner's epoch load and this snapshot — the arm belongs to the old
/// generation and must never back the new turn's commit (cleared here,
/// before any select/RPC, so the rule is unit-testable).
#[allow(clippy::too_many_arguments)]
pub async fn settle_step(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    status: &str,
    acc: &mut Vec<String>,
    retry_wait: &mut u64,
    settled_since: &mut SettledArm,
    fatal_stuck: bool,
    loop_epoch: u64,
) -> SettleStep {
    // Collapse done↔idle flapping before committing to a report. A
    // failed recheck is unknown, not settled: clear the timer (the
    // caller's herdr-error arm does the same) so a post-outage sample
    // never counts as persistence spanning the blackout. A settled-kind
    // flip (done→blocked→idle) also clears: persistence of one kind is
    // not persistence of another. A supersede/cancel landing during the
    // sleep owns the pane even when the kind is unchanged (fast
    // done→done): the pre-sleep epoch below catches it.
    // Single-epoch entry triple (finalize parity): generation (epoch +
    // pending, atomic under one lock) + prompt text. Threaded through
    // the remap and into finalize — finalize must NEVER re-snapshot. A
    // submit landing anywhere after this entry (sleep, recheck RPCs,
    // remap, or the cross-thread gap before finalize's first line)
    // bumps the epoch, and every gate below compares against entry_epoch,
    // so the old acc/screen can never post as the new generation.
    // Prompt/enqueue skew: the atomic `snapshot_entry` (same lock order
    // as `publish_submit`) pairs the generation with the prompt text —
    // a split read could else pair old-gen with new-prompt (or vice
    // versa). The pre-epoch check below still aborts on any move across
    // the snapshot itself.
    let pre_epoch = job.epoch.load(Ordering::Relaxed);
    let (entry_epoch, entry_pending, entry_prompt) = job.snapshot_entry().await;
    // Live-slot entry (retire parity): the shared transient's (mid,
    // generation) at entry — a submit landing anywhere after this (like
    // the job entry above) claims the slot, and the retire gate below
    // stands the stale delete down. Snapshot alongside the job entry so
    // both pin the same generation; finalize must NEVER re-snapshot.
    let live_entry = s.live_get(pane).await.map(|l| (l.mid, l.turn));
    // Generation-bound arm (runner last_epoch parity): a submit landing
    // after the runner's loop-top load but before this snapshot moves
    // entry_epoch off loop_epoch — the arm was armed under the OLD
    // generation and must not count as persistence for the new turn.
    if entry_epoch != loop_epoch {
        *settled_since = None;
        return SettleStep::Continue;
    }
    if pre_epoch != entry_epoch
        || job.epoch.load(Ordering::Relaxed) != entry_epoch
        || job.is_stopped()
    {
        *settled_since = None;
        return SettleStep::Continue;
    }
    // Watcher spawn races the submit RPC (enqueue spawns before
    // publish_submit): a settled idle screen with nothing owed would
    // finalize the pre-submit tail (empty prompt → screen takeover) and
    // settle_books with pending==0. Wait for cover; clear any arm that
    // accrued while still unpaid so it never commits post-publish.
    if entry_pending == 0 {
        *settled_since = None;
        return SettleStep::Continue;
    }
    let epoch_at_entry = entry_epoch;
    // Cancellable like every backoff below: /cancel or a superseding
    // prompt landing inside the 750ms window must not stall its handoff.
    tokio::select! {
        _ = job.cancel.notified() => {
            // Loud /cancel only (quiet retires never notify): the card
            // must read cancelled, not silently drop. Single source with
            // the runner's cancel arm and the backoff arm below.
            *settled_since = None;
            super::runner_cancel::cancel_watch_parts(s, pane, job).await;
            return SettleStep::Break;
        }
        _ = sleep_or_superseded(job, epoch_at_entry, Duration::from_millis(750)) => {}
    }
    // A failed submit retires via mark_stopped() with no epoch bump and
    // no notify (enqueue.rs): an epoch-only check is blind to it and
    // would finalize a retired job's stale screen into a bogus card.
    if job.epoch.load(Ordering::Relaxed) != epoch_at_entry || job.is_stopped() {
        *settled_since = None;
        return SettleStep::Continue;
    }
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
            eprintln!(
                "[watcher] {pane} confirming read failed: {}",
                crate::types::mask_home(&e.to_string())
            );
            *settled_since = None;
            return SettleStep::Continue;
        }
        _ => {}
    }
    // Time-based confirmation: the first settled sample arms the timer
    // (recording its kind), only same-kind persistence commits. Sample
    // counting alone retires on two sub-second event wakes inside one
    // transient gap. Blocked skips the 5s gate — input is needed NOW and
    // blocked is never a mid-run transient (the 750ms recheck above
    // already filtered blips).
    if !settle_commit(status, settled_since, Instant::now(), fatal_stuck) {
        return SettleStep::Continue;
    }
    let epoch_before = entry_epoch;
    repoint_dest_if_remapped(s, pane, job, epoch_before).await;
    // A submit landing during the remap fold owns the pane: finalize
    // captures its entry epoch AFTER the remap RPCs and would mistake
    // the old status/acc for the new prompt's — stop here instead.
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        *settled_since = None;
        return SettleStep::Continue;
    }
    let retry = finalize(
        s,
        pane,
        job,
        status,
        acc,
        (entry_epoch, entry_pending, entry_prompt, live_entry),
    )
    .await;
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
                // Like the sibling cancel branch: a genuine cancel
                // retires the durable intent (a supersede never
                // notifies — it bumps the epoch instead).
                super::runner_cancel::cancel_watch_parts(s, pane, job).await;
                return SettleStep::Break;
            }
            _ = sleep_or_superseded(job, epoch_now, Duration::from_secs(*retry_wait)) => {}
        }
        *retry_wait = (*retry_wait * 2).min(60);
        // The backoff above is a blackout (no samples): an arm timestamped
        // before it must not count as persistence spanning it — the next
        // settled sample re-arms instead of committing at once.
        *settled_since = None;
        return SettleStep::Continue;
    }
    // Turn complete with prompts held: serve the next queued item as a
    // fresh turn (own submit, books, and final) instead of retiring.
    if super::queue::serve_next(s, pane, job).await {
        return SettleStep::Continue;
    }
    SettleStep::Break
}

/// Sleep up to `dur`, returning early when the job's epoch moves
/// (a superseding prompt took over mid-backoff) or the job is stopped
/// (a failed submit retired it with no notify and no epoch bump — the
/// loop top would else wait out the full backoff before exiting).
pub(crate) async fn sleep_or_superseded(job: &Arc<Job>, epoch: u64, dur: Duration) {
    let end = Instant::now() + dur;
    while job.epoch.load(Ordering::Relaxed) == epoch && !job.is_stopped() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        tokio::time::sleep(left.min(Duration::from_millis(250))).await;
    }
}

#[cfg(test)]
#[path = "settle_tests.rs"]
mod tests;
