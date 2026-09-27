//! Instant progress message for prompt turns: ONE silent working message
//! PER TURN (placeholder on submit, that turn's transient tail edited in
//! place after), retired when that turn's buzzing final lands as a NEW
//! message (deleted with `/transient on`, kept as history when off). A
//! turn never borrows the previous turn's message — every user message
//! gets its own, so a second message can never render onto the first's.
//! After the very first muted entry, every update is an edit (edits
//! never notify) — only finals buzz, so the notification shade holds
//! finals alone.
//!
//! Fail-closed: every send/edit/delete is best-effort (a miss retries
//! next tick, a gone message frees the slot). No lock is held across an
//! RPC — snapshot, drop, then call. Retire paths live in
//! `progress_retire` (300-line file limit), re-exported below.
use crate::{
    jobs::job::Job,
    state::{AppState, live::LiveSlot},
    telegram::errors::edit_gone,
    types::MAX_MSG_UNITS,
    ui::tail_fit,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use super::progress_instant::ensure_instant;
pub use super::progress_retire::{clear_live, clear_live_if_epoch, clear_live_if_ownerless};
use super::progress_retire::{is_owner, post_fresh};

/// First instant reply: covers easy turns with no transient output yet.
pub const THINKING: &str = "💭 thinking…";
/// Min gap between progress edits (ticks run every ~2s).
const EDIT_COOLDOWN_SECS: u64 = 4;
/// Header reserve inside the Telegram size cap.
const TAIL_RESERVE: usize = 64;

/// Render the progress body: placeholder when nothing streamed yet,
/// else the bare model tail — chrome-filtered (TUI footers, status
/// bars, tool echoes, spinners are the agent's furniture, never its
/// reply: sending them spams every tick as token counts churn), with
/// no header (the "working" line is our guide, not agent output —
/// nice for the start, removed once real text streams). Finals are
/// untouched (arbitration cleans at display time); only the transient
/// renders through the filter, so prose survives byte-for-byte by the
/// filter/prose battery.
pub fn render_progress(acc: &[String]) -> String {
    if acc.is_empty() {
        return THINKING.to_string();
    }
    let clean = super::filter::chrome_filtered(acc);
    let tail = tail_fit(&clean, MAX_MSG_UNITS.saturating_sub(TAIL_RESERVE));
    if tail.is_empty() {
        return THINKING.to_string();
    }
    tail
}

/// No-op edit verdict (pure, tested): Telegram rejects an edit that
/// changes nothing, so "already showing this" must read as converged —
/// treating it as a failure made every turn log a bogus error.
pub(crate) fn not_modified(err: &str) -> bool {
    crate::telegram::errors::topic_not_modified(err)
}

/// Next attempt time after a failed live RPC (pure, tested): a
/// flood-wait banks its full window (retrying inside `retry after N`
/// only extends the ban), anything else backs off to
/// `EDIT_RETRY_BACKOFF_SECS` so a persistently failing slot retries at
/// a bounded rate instead of once per tick.
pub(crate) fn retry_at(now: &Instant, err: &str) -> Instant {
    let wait = crate::telegram::TelegramClient::retry_after(err)
        .unwrap_or(Duration::from_secs(EDIT_RETRY_BACKOFF_SECS));
    *now + wait
}

/// Min gap between progress edits (ticks run every ~2s).
const EDIT_RETRY_BACKOFF_SECS: u64 = 15;

/// Pure edit gate (tested): new text only, throttled to the cooldown.
/// A first edit (no prior attempt) always goes through. Saturating:
/// a flood-wait banks a FUTURE attempt time, which must read as
/// throttled, never panic.
pub(crate) fn edit_due(last: &str, next: &str, last_at: Option<Instant>, now: Instant) -> bool {
    if last == next {
        return false;
    }
    // Never edit TO the placeholder: an empty acc renders as THINKING, and
    // overwriting delivered output with it is the reported data loss.
    if next == THINKING {
        return false;
    }
    // First content render after a placeholder bypasses the cooldown (reset
    // parity): the placeholder's `at` starts the clock, so a throttled
    // first tail left the user on a bare "thinking…" for a whole cooldown.
    // Bounded to one per turn: after it, `last` is no longer the
    // placeholder, so the gate below is back in force.
    if last == THINKING {
        return true;
    }
    match last_at {
        None => true,
        Some(t) => now.saturating_duration_since(t) >= Duration::from_secs(EDIT_COOLDOWN_SECS),
    }
}

/// Live update after streaming: post when slotless, adopt the new dest
/// on remap, else edit on new throttled text. Detached watchers (map
/// holds another Arc) and stopped jobs never create traffic.
pub async fn refresh_live(s: &AppState, pane: &str, job: &Arc<Job>, acc: &[String]) {
    if job.is_stopped() {
        return;
    }
    if !is_owner(s, pane, job).await {
        return;
    }
    let next = render_progress(acc);
    let dest = *job.dest.lock().await;
    let now = Instant::now();
    match s.live_get(pane).await {
        // Slotless or banked miss: post when the gate allows (a deleted
        // thread fails every send — attempts back off to the cooldown
        // instead of firing each ~2s tick). Slotless posts start a
        // fresh lineage (no retire can hold a generation for a slot
        // that does not exist — gates pair mid+turn, and the fresh mid
        // never matches).
        None => post_fresh(s, pane, job, dest, &next, 0).await,
        Some(sl) if sl.mid < 0 => {
            if edit_due(&sl.text, &next, sl.at, now) {
                // Take winner posts (a successor's fresh slot survives).
                if s.live_take_if(pane, sl.mid, sl.turn).await.is_none() {
                    return;
                }
                post_fresh(s, pane, job, dest, &next, sl.turn + 1).await;
            }
        }
        Some(sl) if (sl.chat, sl.thread) != dest => {
            println!(
                "[live] remap {pane} m{}: dropping corpse-thread msg",
                sl.mid
            );
            s.tg.delete_msg(sl.chat, sl.mid).await;
            s.forget_target(sl.chat, sl.mid).await;
            if s.live_take_if(pane, sl.mid, sl.turn).await.is_none() {
                return;
            }
            post_fresh(s, pane, job, dest, &next, sl.turn + 1).await;
        }
        Some(sl) => {
            if !edit_due(&sl.text, &next, sl.at, now) {
                return;
            }
            let res = s.tg.try_edit_msg(sl.chat, sl.mid, &next, None).await;
            let emsg = res
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            if res.is_ok() || not_modified(&emsg) {
                s.live_put(
                    pane,
                    LiveSlot {
                        text: next,
                        at: Some(now),
                        ..sl
                    },
                )
                .await;
            } else if edit_gone(&emsg) {
                // Dead message: free the slot, repost on the next tick
                // (post_fresh needs the take, and a re-post here would
                // race the next tick's own post).
                println!("[live] edit {pane} m{} gone, slot freed", sl.mid);
                s.live_take_if(pane, sl.mid, sl.turn).await;
                s.forget_target(sl.chat, sl.mid).await;
            } else {
                // Fail-visible + bounded backoff (never silent): a blind
                // retry every tick both hid the failure and re-hit
                // flood-waits inside their own window.
                println!(
                    "[live] edit {pane} m{} failed: {}",
                    sl.mid,
                    crate::types::mask_home(&emsg)
                );
                s.live_put(
                    pane,
                    LiveSlot {
                        at: Some(retry_at(&now, &emsg)),
                        ..sl
                    },
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
