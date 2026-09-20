//! Shared cancel retire for prompt watchers. Split from `runner`
//! (300-line file limit): mark, clear the intent only when the map
//! still points here (a superseding enqueue owns it otherwise), fold
//! the live card quiet. Single source for the select arm, the backoff
//! arm, and the reconnect race.
use crate::{
    jobs::finalize::edit_live, jobs::job::Job, jobs::live::LiveSlot,
    jobs::report::{CANCELLED, cancel_owns_intent},
    state::AppState,
};
use std::sync::Arc;

pub(crate) async fn cancel_watch(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    live: &mut LiveSlot,
) {
    cancel_watch_parts(s, pane, job, &mut live.mid, &mut live.dest).await;
}

/// Split-fields variant for `settle_step` (holds `live_mid`/`live_dest`,
/// not the slot): same single source, no inline copies.
pub(crate) async fn cancel_watch_parts(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
) {
    job.mark_stopped();
    if cancel_owns_intent(s.jobs.lock().await.get(pane), job) {
        s.clear_pending(pane).await;
    }
    let (chat, th) = *job.dest.lock().await;
    edit_live(s, chat, th, pane, live_mid, live_dest, CANCELLED).await;
}
