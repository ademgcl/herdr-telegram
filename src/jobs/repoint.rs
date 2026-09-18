//! Mid-job topic remap: paced reset migrates the topic + deletes the
//! old thread. Split from `settle` (300-line file limit).
use crate::{
    jobs::job::Job,
    state::AppState,
    types::LIVE_RPC_TIMEOUT_SECS,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::time::Duration;

/// Repoint dest at the live thread, or delivery retries into the corpse
/// forever. Folds the old-thread live card (or it freezes on "working…")
/// and drops its address; write-back keeps runner ticks,
/// settle_books and durable intent on the same address.
/// Forum-topic jobs only (DM dests must never gain a thread); epoch +
/// pending guarded like the reconcile restores so a submit racing the
/// migration wins over the corpse's text.
pub async fn repoint_dest_if_remapped(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    epoch_before: u64,
) {
    let Some(forum) = s.cfg.forum else {
        return;
    };
    // A submit landing during the 750ms recheck + confirm owns the pane
    // now — touch nothing (finalize's supersede checks retire us).
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return;
    }
    let Some(cur) = s.topics.storage.get_thread(pane) else {
        return;
    };
    let (chat, old_th) = *job.dest.lock().await;
    if chat != forum {
        return;
    }
    let Some(old) = old_th else {
        return;
    };
    if old == cur {
        return;
    }
    // Re-validate under the write: a submit racing the reads above wins.
    let mut dest = job.dest.lock().await;
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return;
    }
    if *dest != (chat, Some(old)) {
        return;
    }
    *dest = (chat, Some(cur));
    drop(dest);
    // Retire the old-thread card where it lives (live_dest), never the
    // new dest — or the old thread freezes on "working…" while the reply
    // lands in the new one. Fail-closed: mid without dest drops. Bounded
    // like the handoff retire — a slow Telegram must not stall settle
    // past the tick (a miss heals next tick via the slot rules).
    if let Some(mid) = live_mid.take() {
        if let Some((lchat, _)) = live_dest.take() {
            let _ = tokio::time::timeout(
                Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
                s.tg.try_edit_msg(lchat, mid, "🔄 continued in new topic", None),
            )
            .await;
        }
    } else {
        live_dest.take();
    }
    // Migrate the durable only when it still holds the pre-migration
    // intent (or nothing): a racing submit's newer text wins — atomic
    // check-and-set, never check-then-remember across awaits.
    let prompt = job.prompt.lock().await.clone();
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return;
    }
    s.remember_pending_cas(pane, (chat, Some(old), &prompt), (chat, Some(cur), &prompt))
        .await;
}
