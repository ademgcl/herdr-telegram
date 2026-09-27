//! New-turn claim on the shared slot (split from `progress`: 300-line
//! file limit). Posts the silent placeholder before the slow submit RPC,
//! and decides whether the existing slot is reused or superseded.
use crate::{
    jobs::{
        job::Job,
        progress::{THINKING, not_modified, retry_at},
    },
    state::{AppState, live::LiveSlot},
    telegram::errors::edit_gone,
};
use std::sync::Arc;
use std::time::Instant;

use super::progress_retire::post_fresh;

/// New-turn claim on the shared slot: a BARE placeholder is edited back
/// to the placeholder (the old tail never poses as the new turn) and
/// bumps the generation (a stale retire gated on the old one stands
/// down); delivered output is never overwritten (see the arm below); a
/// changed dest drops the stale thread's message and posts fresh. Silent
/// (no buzz) — the final owns the notification. Best-effort: a miss
/// retries on the next stream tick.
pub async fn ensure_instant(s: &AppState, pane: &str, job: &Arc<Job>) {
    let dest = *job.dest.lock().await;
    let now = Instant::now();
    match s.live_get(pane).await {
        Some(sl) if (sl.chat, sl.thread) == dest && sl.mid >= 0 => {
            if sl.text != THINKING {
                // Delivered output in the slot: KEEP it, post this turn's
                // own placeholder. Resetting it destroyed the previous
                // reply in place — the reported "my second message deleted
                // the first reply and replaced it with thinking…", which
                // then also sat stuck on the placeholder forever (a
                // superseded turn's accumulation is dropped, so nothing
                // refilled it). With `/transient` off that message IS the
                // user's copy of the reply: history from now on. No
                // liveness probe needed — the action is the same whether
                // it is alive or gone.
                println!(
                    "[live] supersede {pane} m{}: keeps delivered output, posting fresh",
                    sl.mid
                );
                post_fresh(s, pane, job, dest, THINKING, sl.turn + 1).await;
                return;
            }
            // Same message, new turn: reset + claim the generation.
            // The reset ALWAYS runs — it is the liveness probe for the
            // slot: a message the user deleted (or one that aged out)
            // must be detected and re-posted, never trusted because it
            // still looks like the placeholder (the old early-return
            // left a dead slot looking alive, so the turn ran with no
            // instant feedback at all).
            //
            // Bypasses the edit cooldown: at most one attempt per turn
            // (bounded, never churn), while a throttled reset leaves the
            // previous turn's tail posing as the new turn's status — and
            // a fast-settling turn ends before any tick retries.
            let res = s.tg.try_edit_msg(sl.chat, sl.mid, THINKING, None).await;
            let emsg = res
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            if res.is_ok() || not_modified(&emsg) {
                // Converged (edited, or already the placeholder —
                // Telegram reports the no-op edit; logging it would
                // spam every turn).
                if sl.text != THINKING {
                    println!("[live] reset {pane} m{} to placeholder", sl.mid);
                }
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
            } else if edit_gone(&emsg) {
                // Dead message (deleted by the user, or aged out):
                // free the slot and post fresh so the turn is never
                // silently instant-less.
                println!("[live] reset {pane} m{} gone, repost fresh", sl.mid);
                s.live_take_if(pane, sl.mid, sl.turn).await;
                s.forget_target(sl.chat, sl.mid).await;
                post_fresh(s, pane, job, dest, THINKING, sl.turn + 1).await;
            } else {
                // Fail-visible + flood-aware: a silent retry here left
                // the user staring at a typing indicator with no message.
                println!(
                    "[live] reset {pane} m{} failed: {}",
                    sl.mid,
                    crate::types::mask_home(&emsg)
                );
                s.live_put(
                    pane,
                    LiveSlot {
                        at: Some(retry_at(&now, &emsg)),
                        turn: sl.turn + 1,
                        ..sl
                    },
                )
                .await;
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
