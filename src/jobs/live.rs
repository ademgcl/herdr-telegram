//! Live-message streaming: fresh output edits one Telegram card,
//! silent until the buzzing final (see `report`/`finalize`).
use super::report::WORKING_HEAD;
use crate::{
    jobs::{job::Job, segment::final_block, stream::delta},
    state::AppState,
    types::{LIVE_EDIT_COOLDOWN_SECS, LIVE_RPC_TIMEOUT_SECS},
    ui::tail_fit,
};
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;
/// Live-card tail width: fits Telegram limits with the working head.
const LIVE_TAIL: usize = 3200;

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
    // Instant sustain alongside content: spawned, never awaited, so a
    // slow send never delays the live edit below.
    {
        let tg = s.tg.clone();
        tokio::spawn(async move {
            tg.typing(chat, th).await;
        });
    }
    let text = format!("{WORKING_HEAD}\n\n{}", tail_fit(&seg, LIVE_TAIL));
    // Silent progress: the finish buzzes, streaming never does. Only a
    // landed card owns the slot + throttle, or an outage would orphan
    // the slot address and suppress the next tick.
    // Edit address comes from the slot itself (remap-safe), never
    // job.dest. A fresh send happens ONLY when the old card is
    // definitely gone (deleted topic/message, lost rights) — never two
    // working cards. Any other edit failure (timeout, flood-wait,
    // network) keeps the slot and retries next tick: the card is
    // plausibly still alive, and a fresh send now would duplicate it.
    if let Some(mid) = live.mid {
        // Fail-closed split: mid without dest drops without editing
        // rather than guessing the thread after a remap — and consumes
        // the whole orphaned address, or every later tick would wedge
        // on this return instead of sending fresh at the current dest.
        let Some((edit_chat, _)) = live.dest else {
            live.mid = None;
            live.dest = None;
            return;
        };
        match tokio::time::timeout(
            Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
            s.tg.try_edit_msg(edit_chat, mid, &text, None),
        )
        .await
        {
            Ok(Ok(())) => {
                // Slot address already correct — only the throttle moves.
                live.last_edit = Instant::now();
                return;
            }
            Ok(Err(e)) if crate::telegram::messages::edit_gone(&e.to_string()) => {}
            Ok(Err(_)) | Err(_) => return,
        }
    }
    // No slot, or the old card is definitely gone: one fresh silent
    // card (bounded inside send_silent; a miss retries next tick).
    // Awaited directly with no outer timeout so a card delivered before
    // the deadline is always tracked (see send_silent). Residual race, documented not
    // solved: a send that lands server-side after our deadline timed out
    // duplicates on retry — Telegram offers no idempotency key, and the
    // edit path (which can always retry in place) never duplicates.
    if let Some(m) = s.tg.send_silent(chat, th, &text).await {
        live.mid = Some(m);
        live.dest = Some((chat, th));
        live.last_edit = Instant::now();
    }
}
