//! New-turn claim on the slot (split from `progress`: 500-line file
//! limit). Posts this turn's own silent placeholder before the slow
//! submit RPC. A turn NEVER borrows the previous turn's message.
use crate::{
    jobs::{job::Job, progress::THINKING},
    state::AppState,
};
use std::sync::Arc;

use super::progress_retire::post_fresh;

/// New-turn claim on the slot: post THIS turn's own placeholder, silent
/// (no buzz — the final owns the notification), before the slow submit
/// RPC so the feedback is instant.
///
/// A turn owns its transient outright: minted here, edited in place with
/// that turn's tail, retired when its final lands. The previous turn's
/// message is never reused. Editing it back to the placeholder instead —
/// the old behavior for a slot still showing a bare `💭 thinking…` — is
/// what made a second Telegram message render onto the first turn's
/// message ("the second msg overrode the first, same transient with the
/// wrong history"). It is also the only case where reuse was safe, since
/// a bare placeholder looks like it holds no history; it still reads as
/// one message per turn in the chat, so it was never safe.
///
/// The generation still bumps (`turn + 1`) so a stale retire gated on
/// the old entry stands down. The old message is left exactly as it was:
/// with `/transient` off it IS the user's history, and with it on the
/// previous turn's own retire deletes it. Best-effort — a miss banks an
/// unsent slot and the next stream tick retries.
pub async fn ensure_instant(s: &AppState, pane: &str, job: &Arc<Job>) {
    // Working messages are off by default (`/transient` to enable): the
    // typing indicator carries progress and the final carries the reply,
    // so no message is posted at all. The edits that follow a working
    // message cannot be muted — Telegram's `editMessageText` has no
    // `disable_notification` — so posting one buzzes the phone however
    // quietly the first send was.
    if !s.transient_on() {
        return;
    }
    let dest = *job.dest.lock().await;
    match s.live_get(pane).await {
        // Any live slot on this dest is an EARLIER turn's message: post
        // fresh beside it, never on top of it.
        Some(sl) if (sl.chat, sl.thread) == dest && sl.mid >= 0 => {
            println!(
                "[live] new turn {pane}: m{} was the previous turn's, posting own",
                sl.mid
            );
            post_fresh(s, pane, job, dest, THINKING, sl.turn + 1).await;
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
