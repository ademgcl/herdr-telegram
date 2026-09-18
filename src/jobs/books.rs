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
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        let mut p = job.pending.lock().await;
        *p = p.saturating_sub(entry_pending);
        return;
    }
    *job.pending.lock().await = 0;
    // Clear the durable intent only if it still belongs to this prompt:
    // a shell command (or a re-entered agent prompt) may have
    // overwritten the shared per-pane slot mid-finalize — wiping it
    // would eat their reply's intent while ours is already delivered.
    let prompt = job.prompt.lock().await.clone();
    let (chat, th) = *job.dest.lock().await;
    if s.pending_matches(pane, chat, th, &prompt).await {
        s.clear_pending(pane).await;
    }
    let mut map = s.jobs.lock().await;
    if map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
        map.remove(pane);
        println!("[prompt] watcher retired: {pane}");
    }
}
