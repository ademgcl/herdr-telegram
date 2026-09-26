//! Watcher bookkeeping: cover prompts owed at entry so a new submit
//! mid-finalize keeps its pending count, persisted intent and map
//! entry. Split from `finalize` (300-line file limit). Pure
//! relocation — epoch/pending/intent/map semantics unchanged.
use crate::{jobs::job::Job, state::AppState};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Cover the prompts owed at entry. A new submit mid-finalize bumps the
/// epoch: leave its pending count, persisted intent and map entry so the
/// watcher loop keeps serving it.
pub async fn settle_books(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    entry_epoch: u64,
    entry_pending: usize,
) {
    // Remove ONLY our entry share, never zero blindly: the snapshot
    // and the submit publish share the pending lock (see publish_submit),
    // so the entry pair is always consistent — but a submit landing
    // between the epoch check and this write still grows the count, and
    // a blind zero would eat the new prompt's cover (a later failed
    // submit would read owed==0 and retire the live watcher + wipe the
    // durable intent — silent prompt loss). Saturating-sub keeps a
    // concurrent +1 alive on both paths.
    {
        let mut p = job.pending.lock().await;
        *p = p.saturating_sub(entry_pending);
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            return;
        }
    }
    // Clear the durable intent only if it still belongs to this prompt:
    // a shell command (or a re-entered agent prompt) may have
    // overwritten the shared per-pane slot mid-finalize — wiping it
    // would eat their reply's intent while ours is already delivered.
    // Match-guarded (same TOCTOU as the shell settle): the check above
    // and the clear below span awaits.
    let prompt = job.prompt.lock().await.clone();
    let (chat, th) = *job.dest.lock().await;
    // Re-check after the snapshots above: a submit interleaving here
    // owns the pane now — wiping its intent loses the reply. Our share
    // already left the count above, so this is a bare return (subtracting
    // again would eat the newcomer's cover — see above).
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        return;
    }
    // Atomic owner-checked clear (jobs ptr_eq + epoch + text under ONE
    // jobs→pending hold, retire.rs order): the old detached successor
    // guard released the jobs lock before the clear, so a failed-submit
    // retire (mark_stopped + map remove, NO epoch bump) + same-text
    // re-prompt landing in that window matched text+epoch and wiped the
    // successor's intent (silent reply loss). Vacant (already retired)
    // or self-owned slots still clear; a mapped successor refuses inside.
    // Shell parity: clear_shell_if_matches.
    s.clear_pending_if_epoch_matches(pane, job, chat, th, &prompt, entry_epoch)
        .await;
    // Re-check the epoch under the map guard with no await after: reuse
    // keeps the SAME Arc (ptr_eq alone cannot tell a successor apart),
    // so a submit landing between the checks above and this remove
    // would else lose its map entry from under a live watcher.
    let mut map = s.jobs.lock().await;
    if job.epoch.load(Ordering::Relaxed) == entry_epoch
        && map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false)
    {
        map.remove(pane);
        println!("[prompt] watcher retired: {pane}");
    }
}
