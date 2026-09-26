//! Instant progress message for prompt turns: the pane's ONE silent
//! working message (placeholder on first submit, transient tail edited
//! in place after that — thinking, tool echoes, progress verbs), then
//! retired when the buzzing final lands as a NEW message (deleted with
//! `/transient on`, kept as history when off). After the very first
//! muted entry, every update is an edit (edits never notify) — only
//! finals buzz, so the notification shade holds finals alone.
//!
//! Fail-closed: every send/edit/delete is best-effort (a miss retries
//! next tick, a gone message frees the slot). No lock is ever held
//! across an RPC — snapshot, drop, then call. Retire paths live in
//! `progress_retire` (300-line file limit), re-exported below so call
//! sites keep `progress::…`.
use crate::{
    jobs::job::Job,
    state::{AppState, live::LiveSlot},
    telegram::errors::edit_gone,
    types::MAX_MSG_UNITS,
    ui::tail_fit,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use super::progress_retire::{clear_live, clear_live_if_epoch, clear_live_if_ownerless};
use super::progress_retire::{is_owner, post_fresh};

/// First instant reply: covers easy turns with no transient output yet.
pub const THINKING: &str = "💭 thinking…";
/// Live header once streamed output exists (tail follows below it).
const WORKING_HEAD: &str = "💭 working…";
/// Min gap between progress edits (ticks run every ~2s).
const EDIT_COOLDOWN_SECS: u64 = 4;
/// Header reserve inside the Telegram size cap.
const TAIL_RESERVE: usize = 64;

/// Render the progress body: placeholder when nothing streamed yet,
/// else the header plus the raw tail (unfiltered — transient lines are
/// the point here; the final still arbitrates its clean reply).
pub fn render_progress(acc: &[String]) -> String {
    if acc.is_empty() {
        return THINKING.to_string();
    }
    let tail = tail_fit(acc, MAX_MSG_UNITS.saturating_sub(TAIL_RESERVE));
    if tail.is_empty() {
        return THINKING.to_string();
    }
    format!("{WORKING_HEAD}\n\n{tail}")
}

/// Pure edit gate (tested): new text only, throttled to the cooldown.
/// A first edit (no prior attempt) always goes through. Saturating:
/// a flood-wait banks a FUTURE attempt time, which must read as
/// throttled, never panic.
pub(crate) fn edit_due(last: &str, next: &str, last_at: Option<Instant>, now: Instant) -> bool {
    if last == next {
        return false;
    }
    match last_at {
        None => true,
        Some(t) => now.saturating_duration_since(t) >= Duration::from_secs(EDIT_COOLDOWN_SECS),
    }
}

/// New-turn claim on the shared slot: same dest edits back to the
/// placeholder (the old tail never poses as the new turn) and bumps
/// the generation (a stale retire gated on the old one stands down);
/// a changed dest drops the stale thread's message and posts fresh.
/// Silent (no buzz) — the final owns the notification. Best-effort: a
/// miss retries on the next stream tick.
pub async fn ensure_instant(s: &AppState, pane: &str, job: &Arc<Job>) {
    let dest = *job.dest.lock().await;
    let now = Instant::now();
    match s.live_get(pane).await {
        Some(sl) if (sl.chat, sl.thread) == dest && sl.mid >= 0 => {
            // Same message, new turn: reset + claim the generation.
            // Skip when already showing it (fresh post above).
            if sl.text == THINKING {
                return;
            }
            if !edit_due("", THINKING, sl.at, now) {
                // Throttled: still claim the generation now (a stale
                // retire must stand down even though the reset text
                // lands on the next tick).
                s.live_put(
                    pane,
                    LiveSlot {
                        turn: sl.turn + 1,
                        ..sl
                    },
                )
                .await;
                return;
            }
            match s.tg.try_edit_msg(sl.chat, sl.mid, THINKING, None).await {
                Ok(()) => {
                    println!("[live] reset {pane} m{} to placeholder", sl.mid);
                    s.live_put(
                        pane,
                        LiveSlot {
                            text: THINKING.to_string(),
                            at: Some(now),
                            turn: sl.turn + 1,
                            ..sl
                        },
                    )
                    .await;
                }
                Err(e) if edit_gone(&e.to_string()) => {
                    println!("[live] reset {pane} m{} gone, slot freed", sl.mid);
                    s.live_take_if(pane, sl.mid, sl.turn).await;
                    s.forget_target(sl.chat, sl.mid).await;
                }
                Err(_) => {
                    s.live_put(
                        pane,
                        LiveSlot {
                            at: Some(now),
                            turn: sl.turn + 1,
                            ..sl
                        },
                    )
                    .await;
                }
            }
        }
        Some(sl) if sl.mid >= 0 => {
            // Retargeted (remap): the old thread is stale — drop it
            // best-effort, then post fresh below (lineage continues so
            // any in-flight retire gated on it stands down). Only the
            // take winner posts: a successor owning the slot now keeps
            // it, never a duplicate beside it.
            println!("[live] retarget {pane} m{}: dropping stale", sl.mid);
            s.tg.delete_msg(sl.chat, sl.mid).await;
            s.forget_target(sl.chat, sl.mid).await;
            if s.live_take_if(pane, sl.mid, sl.turn).await.is_none() {
                return;
            }
            post_fresh(s, pane, job, dest, THINKING, sl.turn + 1).await;
        }
        // Banked miss (any remaining `Some` here is the mid<0
        // sentinel — real slots matched above): retry the post now
        // (submit-time, single attempt per turn), continuing lineage.
        Some(sl) => post_fresh(s, pane, job, dest, THINKING, sl.turn + 1).await,
        None => post_fresh(s, pane, job, dest, THINKING, 0).await,
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
            match s.tg.try_edit_msg(sl.chat, sl.mid, &next, None).await {
                Ok(()) => {
                    s.live_put(
                        pane,
                        LiveSlot {
                            text: next,
                            at: Some(now),
                            ..sl
                        },
                    )
                    .await;
                }
                Err(e) if edit_gone(&e.to_string()) => {
                    println!("[live] edit {pane} m{} gone, slot freed", sl.mid);
                    s.live_take_if(pane, sl.mid, sl.turn).await;
                    s.forget_target(sl.chat, sl.mid).await;
                }
                Err(_) => {
                    s.live_put(
                        pane,
                        LiveSlot {
                            at: Some(now),
                            ..sl
                        },
                    )
                    .await;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
