//! Fresh-output accumulation for final-card arbitration (see
//! `report`/`finalize`). No live card is ever posted: the only message
//! the user gets is the buzzing final — activity shows via the typing
//! indicator, never a working box.
use crate::{
    jobs::{job::Job, stream::delta},
    state::AppState,
    types::{LIVE_EDIT_COOLDOWN_SECS, LIVE_RPC_TIMEOUT_SECS},
};
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;

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
            // throttles repeats, never the first card). checked_sub:
            // Instant::now() - cooldown panics when the monotonic clock
            // is younger than the cooldown (fresh boot); fall back to
            // now (first output delayed one cooldown, never a panic).
            last_edit: Instant::now()
                .checked_sub(Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS))
                .unwrap_or_else(Instant::now),
        }
    }

    /// New turn on a reused watcher: fresh throttle. A stale `last_edit`
    /// would cooldown-suppress the new prompt's first output for ~4s.
    /// Address clears separately via the superseded retire below so the
    /// old card folds instead of orphaning.
    pub fn rearm(&mut self) {
        self.last_edit = Instant::now()
            .checked_sub(Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS))
            .unwrap_or_else(Instant::now);
    }

    /// Retire the old turn's card on a handoff (bounded, silent): take
    /// the slot only on landed/gone — a transient failure keeps it for
    /// the new turn to adopt instead of duplicating it with a fresh one.
    pub async fn retire_for_handoff(&mut self, s: &AppState) {
        let Some(mid) = self.mid else {
            self.dest = None;
            return;
        };
        let Some((chat, _)) = self.dest else {
            self.mid = None; // split: drop, never guess the thread
            return;
        };
        let retire = tokio::time::timeout(
            Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
            s.tg.try_edit_msg(chat, mid, "🔄 superseded by a newer prompt", None),
        )
        .await;
        let done = match retire {
            Ok(Ok(())) => true,
            Ok(Err(e)) => crate::telegram::messages::edit_gone(&e.to_string()),
            Err(_) => false,
        };
        if done {
            self.mid = None;
            self.dest = None;
        }
    }
}

/// Track whatever is new into the accumulator (silent; feeds the
/// final-card arbitration — finals are byte-identical to before, only
/// the working box is gone).
pub async fn stream_live(
    _s: &AppState,
    job: &Arc<Job>,
    screen: Vec<String>,
    acc: &mut Vec<String>,
    _live: &mut LiveSlot,
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
