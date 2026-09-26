//! Mid-job topic remap: paced reset migrates the topic + deletes the
//! old thread. Split from `settle` (300-line file limit).
use crate::{jobs::job::Job, state::AppState};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Repoint dest at the live thread, or delivery retries into the corpse
/// forever. No card moves (no live card exists — see live.rs); the
/// reply below simply lands in the new topic. Write-back keeps runner
/// ticks, settle_books and durable intent on the same address.
/// Forum-topic jobs only (DM dests must never gain a thread); epoch +
/// pending guarded like the reconcile restores so a submit racing the
/// migration wins over the corpse's text.
pub async fn repoint_dest_if_remapped(s: &AppState, pane: &str, job: &Arc<Job>, epoch_before: u64) {
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
    // Migrate the durable BEFORE the dest write below: a settle_books
    // snapshotting between a split dest-then-durable write sees the new
    // thread with the old intent and clears nothing (ghost re-arm after
    // restart). Occupied-only CAS (never vacant-insert): a vacant slot
    // means cancelled/settled, and minting the corpse text there would
    // resurrect dead work with a fresh 24h clock. A racing submit's newer
    // text wins the CAS — never check-then-remember across awaits.
    let prompt = job.prompt.lock().await.clone();
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return;
    }
    s.migrate_pending_cas(pane, (chat, Some(old), &prompt), (chat, Some(cur), &prompt))
        .await;
    // Re-validate under the write: a submit racing the reads above wins.
    let mut dest = job.dest.lock().await;
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        return;
    }
    if *dest != (chat, Some(old)) {
        return;
    }
    *dest = (chat, Some(cur));
}
