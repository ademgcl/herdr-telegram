//! Instant progress message for prompt turns: a silent placeholder on
//! submit, live in-place edits with the raw stream tail (transient work
//! included — thinking, tool echoes, progress verbs), then deleted when
//! the buzzing final lands as a NEW message. Edits never notify; only
//! the final buzzes, so status can never be missed in the noise.
//!
//! Fail-closed: every send/edit/delete is best-effort (a missed tick
//! retries, a gone message clears the slot). No lock is ever held
//! across an RPC — snapshot, drop, then call.
use crate::{
    jobs::job::Job, state::AppState, telegram::errors::edit_gone, types::MAX_MSG_UNITS,
    ui::tail_fit,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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
/// A first edit (no prior attempt) always goes through.
pub(crate) fn edit_due(last: &str, next: &str, last_at: Option<Instant>, now: Instant) -> bool {
    if last == next {
        return false;
    }
    match last_at {
        None => true,
        Some(t) => now.duration_since(t) >= Duration::from_secs(EDIT_COOLDOWN_SECS),
    }
}

/// Post the instant placeholder when the turn owns no message yet;
/// reset a reused slot to the placeholder so a follow-up turn never
/// wears the old turn's tail. Silent (no buzz) — the final owns the
/// notification. Best-effort: a miss retries on the next stream tick.
pub async fn ensure_instant(s: &AppState, pane: &str, job: &Arc<Job>) {
    let dest = *job.dest.lock().await;
    let slot = *job.live_msg.lock().await;
    match slot {
        Some((c, th, m)) if (c, th) == dest => {
            // Reused slot (follow-up on the same dest): reset to the
            // placeholder so the old tail never poses as the new turn.
            // Skip when already showing it (fresh post above).
            if *job.live_text.lock().await == THINKING {
                return;
            }
            let at = *job.live_at.lock().await;
            if !edit_due("", THINKING, at, Instant::now()) {
                return;
            }
            match s.tg.try_edit_msg(c, m, THINKING, None).await {
                Ok(()) => {
                    *job.live_text.lock().await = THINKING.to_string();
                    *job.live_at.lock().await = Some(Instant::now());
                }
                Err(e) if edit_gone(&e.to_string()) => {
                    *job.live_msg.lock().await = None;
                    // Reset the text gate too: the next render must
                    // repost even when unchanged (same text would else
                    // suppress the repost until output streams).
                    *job.live_text.lock().await = String::new();
                    *job.live_at.lock().await = None;
                    s.forget_target(c, m).await;
                }
                Err(_) => {
                    *job.live_at.lock().await = Some(Instant::now());
                }
            }
        }
        Some((c, _, m)) => {
            // Retargeted (remap/transfer): the old thread is stale —
            // drop it best-effort, then post fresh below.
            s.tg.delete_msg(c, m).await;
            s.forget_target(c, m).await;
            *job.live_msg.lock().await = None;
            post_fresh(s, pane, job, dest, THINKING).await;
        }
        None => post_fresh(s, pane, job, dest, THINKING).await,
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
    let slot = *job.live_msg.lock().await;
    // Post throttle (failed-post parity with the edit gate below): a
    // deleted thread fails every send — attempts back off to the
    // cooldown instead of firing each ~2s tick.
    let last = job.live_text.lock().await.clone();
    let at = *job.live_at.lock().await;
    let now = Instant::now();
    match slot {
        None => {
            if edit_due(&last, &next, at, now) {
                post_fresh(s, pane, job, dest, &next).await;
            }
        }
        Some((c, th, m)) if (c, th) != dest => {
            s.tg.delete_msg(c, m).await;
            s.forget_target(c, m).await;
            *job.live_msg.lock().await = None;
            post_fresh(s, pane, job, dest, &next).await;
        }
        Some((c, _, m)) => {
            if !edit_due(&last, &next, at, now) {
                return;
            }
            match s.tg.try_edit_msg(c, m, &next, None).await {
                Ok(()) => {
                    *job.live_text.lock().await = next;
                    *job.live_at.lock().await = Some(Instant::now());
                }
                Err(e) if edit_gone(&e.to_string()) => {
                    *job.live_msg.lock().await = None;
                    // Reset the text gate too (ensure_instant parity):
                    // the next render must repost even when unchanged.
                    *job.live_text.lock().await = String::new();
                    *job.live_at.lock().await = None;
                    s.forget_target(c, m).await;
                }
                Err(_) => {
                    *job.live_at.lock().await = Some(Instant::now());
                }
            }
        }
    }
}

/// Move the progress slot from a detached job to its live successor
/// (transfer/rearm): same dest adopts in place (no flicker), a changed
/// dest or an already-owned successor drops the stale duplicate. No
/// new traffic — the successor's ticks render from here.
pub async fn adopt_live(s: &AppState, from: &Arc<Job>, to: &Arc<Job>) {
    if Arc::ptr_eq(from, to) {
        return;
    }
    let slot = *from.live_msg.lock().await;
    let Some((c, th, m)) = slot else { return };
    if to.live_msg.lock().await.is_some() {
        from.live_msg.lock().await.take();
        s.tg.delete_msg(c, m).await;
        s.forget_target(c, m).await;
        return;
    }
    let dest = *to.dest.lock().await;
    if (c, th) != dest {
        *from.live_msg.lock().await = None;
        s.tg.delete_msg(c, m).await;
        s.forget_target(c, m).await;
        return;
    }
    let text = from.live_text.lock().await.clone();
    let at = *from.live_at.lock().await;
    *from.live_msg.lock().await = None;
    *to.live_msg.lock().await = Some((c, th, m));
    *to.live_text.lock().await = text;
    *to.live_at.lock().await = at;
}

/// Epoch-gated retire (finalize parity): a successor owning the pane
/// (moved epoch) keeps the slot — only the still-current turn deletes
/// its transient. Self-healing: a submit racing the delete re-posts on
/// its next tick (gone-edit → fresh post).
pub async fn clear_live_if_epoch(s: &AppState, job: &Arc<Job>, entry_epoch: u64) {
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        return;
    }
    let slot = *job.live_msg.lock().await;
    let Some((c, th, m)) = slot else { return };
    s.tg.delete_msg(c, m).await;
    s.forget_target(c, m).await;
    if job.epoch.load(Ordering::Relaxed) == entry_epoch
        && job.live_msg.lock().await.is_some_and(|v| v == (c, th, m))
    {
        *job.live_msg.lock().await = None;
    }
}

/// Unconditional retire (genuine cancel / dead submit with no
/// successor): the slot belongs to nobody else. Idempotent.
pub async fn clear_live(s: &AppState, job: &Arc<Job>) {
    let slot = job.live_msg.lock().await.take();
    let Some((c, _, m)) = slot else { return };
    s.tg.delete_msg(c, m).await;
    s.forget_target(c, m).await;
}

/// Watcher-exit retire: delete only when no successor owns the pane —
/// a live Arc in the map, or a live same-Arc turn (epoch moved without
/// stopping), keeps the slot. A stopped job never owns a successor turn
/// (enqueue never reuses stopped Arcs), so quiet pane-death retires —
/// which bump the epoch themselves — still retire the placeholder.
/// Idempotent with the finalize/cancel clears above.
pub async fn clear_live_if_ownerless(s: &AppState, pane: &str, job: &Arc<Job>, exit_epoch: u64) {
    // Another Arc owns the pane and may still serve: hands off (its own
    // ticks adopted or replaced the slot; deleting here would take a
    // live turn's message). A stopped entry owns nothing — fall through
    // and retire our own slot below.
    if let Some(cur) = s.jobs.lock().await.get(pane).cloned()
        && !Arc::ptr_eq(&cur, job)
        && !cur.is_stopped()
    {
        return;
    }
    // Live same-Arc successor turn (supersede bumps in place): hands off.
    if !job.is_stopped() && job.epoch.load(Ordering::Relaxed) != exit_epoch {
        return;
    }
    clear_live(s, job).await;
}

/// Map-ownership check (pure lock read, never across RPC): a detached
/// watcher must not post into its successor's turn.
async fn is_owner(s: &AppState, pane: &str, job: &Arc<Job>) -> bool {
    s.jobs
        .lock()
        .await
        .get(pane)
        .is_some_and(|j| Arc::ptr_eq(j, job))
}

/// Silent fresh post + reply-route memory (targets only — progress is
/// transient, so it never joins the reset-copy `last_msgs`). Misses
/// stay slotless and retry next tick. Single-flight across tasks (an
/// enqueue instant vs a watcher tick on a reused Arc): concurrent
/// empty-slot posts would double with the loser orphaned — the loser
/// skips and its next tick sees the slot. A post retired mid-send is
/// dropped instead of orphaned.
async fn post_fresh(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    dest: (i64, Option<i64>),
    text: &str,
) {
    let (c, th) = dest;
    if job
        .live_sending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let mid = s.tg.send_silent(c, th, text).await;
    job.live_sending.store(false, Ordering::Release);
    // Stopped mid-send (cancel/fail retired us during the RPC): the slot
    // belongs to nobody — drop the message, never store it.
    if job.is_stopped() {
        if let Some(m) = mid {
            s.tg.delete_msg(c, m).await;
            s.forget_target(c, m).await;
        }
        return;
    }
    match mid {
        Some(m) => {
            *job.live_msg.lock().await = Some((c, th, m));
            *job.live_text.lock().await = text.to_string();
            *job.live_at.lock().await = Some(Instant::now());
            s.remember_reply(c, m, pane).await;
        }
        None => {
            *job.live_at.lock().await = Some(Instant::now());
        }
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
