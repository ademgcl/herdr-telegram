//! Shared cancel retire for prompt watchers. Split from `runner`
//! (300-line file limit): mark, clear the intent only when the map
//! still points here (a superseding enqueue owns it otherwise), then
//! post the cancel card fresh (no live card exists to edit — see
//! live.rs). Single source for the select arm, the backoff arm, and
//! the reconnect race.
use crate::{
    jobs::{job::Job, report::CANCELLED},
    jobs::stream::EvStream,
    state::AppState,
    types::Res,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) async fn cancel_watch(s: &AppState, pane: &str, job: &Arc<Job>) {
    cancel_watch_parts(s, pane, job).await;
}

/// Same single source, called from `settle_step`'s cancel arms.
pub(crate) async fn cancel_watch_parts(s: &AppState, pane: &str, job: &Arc<Job>) {
    // Entry epoch BEFORE the stop: a superseding enqueue reuses this
    // same Arc and bumps in place — the guarded clear below must not
    // eat the successor's intent (ptr_eq alone is blind to it).
    let epoch_at_entry = job.epoch.load(Ordering::Relaxed);
    job.mark_stopped();
    // Atomic owner-checked clear (no detached check-then-clear across
    // awaits): a superseding enqueue landing mid-retire owns the slot.
    s.clear_pending_if_owner(pane, job, epoch_at_entry).await;
    let (chat, th) = *job.dest.lock().await;
    super::report::report(s, chat, th, pane, CANCELLED).await;
}

/// Event-dial outcome: cancel arms share the runner's retire path,
/// supersede skips the dial (the loop-top epoch arm serves the new
/// turn), opened carries the dial result.
pub(crate) enum DialOut {
    Cancelled,
    Superseded,
    Opened(Res<EvStream>),
}

/// Reopen outcome for the runner's lazy event-stream dial (split from
/// `runner`, 300-line file limit): break retires via the cancel path,
/// cooled skips the dial (loop-top serves the new turn), ready means a
/// stream is connected (or the open failed loudly — polling covers).
pub(crate) enum Reopen {
    Break,
    Cooled,
    Ready,
}

/// Lazy event-stream reconnect, raced against cancel + supersede (see
/// `open_events_raced`). Owns the cooldown stamp: a hung dial counts,
/// so the next attempt waits a full cooldown after it ends instead of
/// retrying immediately.
pub(crate) async fn reopen_events(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    ev: &mut Option<EvStream>,
    last_open: &mut tokio::time::Instant,
) -> Reopen {
    let opened = open_events_raced(s, pane, job, 10).await;
    // Count the dial toward the cooldown (a 10s hung dial must
    // not retry immediately — next attempt 5s after it ends).
    *last_open = tokio::time::Instant::now();
    match opened {
        DialOut::Cancelled => {
            cancel_watch(s, pane, job).await;
            Reopen::Break
        }
        DialOut::Superseded => Reopen::Cooled,
        DialOut::Opened(Ok(stream)) => {
            *ev = Some(stream);
            Reopen::Ready
        }
        DialOut::Opened(Err(e)) => {
            eprintln!(
                "[watcher] {pane} event stream open failed: {}",
                crate::types::mask_home(&e.to_string())
            );
            Reopen::Ready
        }
    }
}

/// Open the event stream raced against cancel + supersede. The open
/// await sits outside the runner's main select, so /cancel must not
/// wait behind up to 10s of dial — and a supersede (epoch bump only,
/// never notify) must cut the wait short the same way instead of
/// stalling the handoff a full dial. The supersede arm completes on
/// change only, never on timeout, so the dial keeps its own bound.
pub(crate) async fn open_events_raced(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    secs: u64,
) -> DialOut {
    let dial_epoch = job.epoch.load(Ordering::Relaxed);
    tokio::select! {
        _ = job.cancel.notified() => DialOut::Cancelled,
        r = EvStream::open_bounded(&s.cfg.socket, pane, secs) => DialOut::Opened(r),
        _ = async {
            while job.epoch.load(Ordering::Relaxed) == dial_epoch && !job.is_stopped() {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        } => DialOut::Superseded,
    }
}
