//! Fresh-output accumulation for final-card arbitration (see
//! `report`/`finalize`). No live card is ever posted: the only message
//! the user gets is the buzzing final — activity shows via the typing
//! indicator, never a working box. (A live-slot address/throttle once
//! lived here; nothing ever posted through it, so it was removed —
//! zero dead code.)
use crate::{
    jobs::{job::Job, stream::delta},
    state::AppState,
};
use std::sync::Arc;

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;

/// Track whatever is new into the accumulator (silent; feeds the
/// final-card arbitration — finals are byte-identical to before, only
/// the working box is gone).
pub async fn stream_live(
    _s: &AppState,
    job: &Arc<Job>,
    screen: Vec<String>,
    acc: &mut Vec<String>,
) {
    if screen.is_empty() {
        return; // nothing readable yet — try next wake-up
    }
    if !job.baseline_ok() {
        job.anchor_baseline(screen).await;
        return;
    }
    let base = job.baseline.lock().await.clone();
    let fresh = delta(&screen, &base);
    if fresh.is_empty() {
        return;
    }
    // Raw accumulation: boundaries (tool echoes, headers, prompt echo)
    // are resolved at display time so only the fresh reply is shown.
    acc.extend(fresh.iter().cloned());
    if acc.len() > ACC_CAP {
        let drop = acc.len() - ACC_CAP;
        acc.drain(..drop);
    }
    *job.baseline.lock().await = screen;
}
