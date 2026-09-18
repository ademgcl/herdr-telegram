//! Live-message streaming: fresh output edits one Telegram card,
//! silent until the buzzing final (see `report`/`finalize`).
use super::report::WORKING_HEAD;
use crate::{
    jobs::{job::Job, segment::final_block, stream::delta},
    state::AppState,
    types::LIVE_EDIT_COOLDOWN_SECS,
    ui::tail_fit,
};
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;
/// Live-card tail width: fits Telegram limits with the working head.
const LIVE_TAIL: usize = 3200;
/// Bound for silent live RPCs (edit/send): flood-wait retries must not
/// stall settle past the tick — a miss retries next tick. Single source
/// for the stream + ack bounds (distinct from the edit cooldown).
pub(crate) const LIVE_RPC_TIMEOUT_SECS: u64 = 4;

/// Live message slot: address (mid+dest) + edit throttle. The runner
/// owns one; stream and folds borrow its fields so the address can
/// never split.
pub struct LiveSlot {
    pub mid: Option<i64>,
    pub dest: Option<(i64, Option<i64>)>,
    pub last_edit: Instant,
}

impl LiveSlot {
    pub fn new() -> Self {
        Self {
            mid: None,
            dest: None,
            // Primed: the first stream lands immediately (cooldown
            // throttles repeats, never the first card).
            last_edit: Instant::now() - Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS),
        }
    }

    /// New turn on a reused watcher: fresh throttle. A stale `last_edit`
    /// would cooldown-suppress the new prompt's first output for ~4s.
    /// Address clears separately via the superseded retire below so the
    /// old card folds instead of orphaning.
    pub fn rearm(&mut self) {
        self.last_edit = Instant::now() - Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS);
    }
}

/// Stream whatever is new into the live message (silent progress; the
/// finish buzzes separately).
pub async fn stream_live(
    s: &AppState,
    job: &Arc<Job>,
    screen: Vec<String>,
    acc: &mut Vec<String>,
    live: &mut LiveSlot,
) {
    if live.last_edit.elapsed() < Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS) {
        return;
    }
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

    let prompt = job.prompt.lock().await.clone();
    let seg = final_block(acc, &prompt);
    if seg.is_empty() {
        return; // chrome-only so far — nothing worth showing yet
    }
    let (chat, th) = *job.dest.lock().await;
    s.tg.typing(chat, th).await;
    let text = format!("{WORKING_HEAD}\n\n{}", tail_fit(&seg, LIVE_TAIL));
    // Silent progress: the finish buzzes, streaming never does. Only a
    // landed card owns the slot + throttle, or an outage would orphan
    // the ack address and suppress the next tick.
    // Edit address comes from the slot itself (remap-safe), never
    // job.dest; fallback posts fresh at the current dest.
    let ech = live.dest.map(|(c, _)| c).unwrap_or(chat);
    if let Some(mid) = live.mid {
        let ok = tokio::time::timeout(
            Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
            s.tg.try_edit_msg(ech, mid, &text, None),
        )
        .await
        .is_ok_and(|r| r.is_ok());
        if ok {
            // Slot address already correct — only the throttle moves.
            live.last_edit = Instant::now();
            return;
        }
    }
    // No slot or edit missed: one fresh silent card (bounded; a miss
    // retries next tick).
    let sent = tokio::time::timeout(
        Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
        s.tg.send_silent(chat, th, &text),
    )
    .await
    .ok()
    .flatten();
    if let Some(m) = sent {
        live.mid = Some(m);
        live.dest = Some((chat, th));
        live.last_edit = Instant::now();
    }
}
