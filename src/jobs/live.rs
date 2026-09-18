//! Live-message streaming: fresh output edits one Telegram card.
//! Split from `runner` (300-line file limit). Pure relocation —
//! cooldown, baseline, delta, adopt-or-send and content tracking
//! behave exactly as before.
use super::report::WORKING_HEAD;
use crate::{
    jobs::{job::Job, segment::final_block, stream::delta},
    state::AppState,
    types::LIVE_EDIT_COOLDOWN_SECS,
    ui::tail_fit,
};
use std::sync::Arc;
use tokio::time::{Duration, Instant};

/// Live message slot: address (mid+dest), content flag (real output
/// vs silent ack), edit throttle. The runner owns one; stream and
/// folds borrow its fields so the address can never split.
pub struct LiveSlot {
    pub mid: Option<i64>,
    pub dest: Option<(i64, Option<i64>)>,
    pub has_content: bool,
    pub last_edit: Instant,
}

impl LiveSlot {
    pub fn new() -> Self {
        Self {
            mid: None,
            dest: None,
            has_content: false,
            // Primed: the first stream lands immediately (cooldown
            // throttles repeats, never the first card).
            last_edit: Instant::now() - Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS),
        }
    }
}

/// Stream whatever is new into the live message. Sets
/// `live_has_content` once the slot holds real output (vs a silent
/// ack): the seed-dup path folds ack-only slots, everything else
/// retires through finalize.
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
    if acc.len() > 400 {
        let drop = acc.len() - 400;
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
    let text = format!("{WORKING_HEAD}\n\n{}", tail_fit(&seg, 3200));
    match live.mid {
        Some(mid) => {
            if s.tg.try_edit_msg(chat, mid, &text, None).await.is_err() {
                live.mid = s.tg.send_msg(chat, th, &text, None).await;
            }
        }
        None => live.mid = s.tg.send_msg(chat, th, &text, None).await,
    }
    live.has_content = live.mid.is_some();
    live.dest = live.mid.map(|_| (chat, th));
    live.last_edit = Instant::now();
}
