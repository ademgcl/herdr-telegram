//! Epoch-guarded retire primitives for cancel paths. Split from `jobs`
//! (300-line file limit): same-Arc reuse bumps the epoch in place, so
//! every retire pins the entry generation — `ptr_eq` alone cannot tell
//! a successor apart (see jobs::books). Lock order jobs→pending
//! everywhere (never inverted: `publish_submit` only ever holds
//! pending, so a racing submit cannot interleave mid-section).
use super::State;
use crate::jobs::persist::{self, PendingPrompt};
use std::sync::Arc;

impl State {
    /// Clear only when the slot still holds what the caller saw at entry
    /// (or is still absent): a submit racing a no-job cancel owns the
    /// slot — wiping it loses a delivered prompt's reply. Single source
    /// for the no-job branches of the cancel paths.
    pub async fn clear_pending_if_unchanged(&self, pane: &str, old: Option<PendingPrompt>) -> bool {
        let snap = {
            let mut map = self.pending.lock().await;
            if map.get(pane) != old.as_ref() {
                return false;
            }
            if map.remove(pane).is_none() {
                return false;
            }
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }

    /// Atomic epoch-guarded retire for cancel paths (single source):
    /// remove the jobs entry AND the durable intent only while the map
    /// still holds this exact Arc at the entry epoch.
    pub async fn cancel_retire_if_epoch(
        &self,
        pane: &str,
        job: &Arc<crate::jobs::job::Job>,
        epoch_at_entry: u64,
    ) -> bool {
        let snap = {
            let mut jobs = self.jobs.lock().await;
            let same = jobs.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false);
            if !same {
                return false;
            }
            let mut pending = self.pending.lock().await;
            if job.epoch.load(std::sync::atomic::Ordering::Relaxed) != epoch_at_entry {
                return false;
            }
            jobs.remove(pane);
            pending.remove(pane);
            pending.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }

    /// Atomic owner-checked clear for cancel retire (single source):
    /// the jobs-map ownership check and the intent removal share ONE
    /// jobs-lock hold with the pending removal inside — a superseding
    /// enqueue landing between a detached check and the clear would else
    /// wipe its intent (check-then-clear across awaits). The verdict
    /// reuses `cancel_owns_intent`, pinned to the entry epoch so
    /// same-Arc reuse (epoch bumped in place) survives.
    pub async fn clear_pending_if_owner(
        &self,
        pane: &str,
        job: &Arc<crate::jobs::job::Job>,
        epoch_at_entry: u64,
    ) -> bool {
        let snap = {
            let jobs = self.jobs.lock().await;
            if !crate::jobs::report::cancel_owns_intent(jobs.get(pane), job, epoch_at_entry) {
                return false;
            }
            let mut pending = self.pending.lock().await;
            if pending.remove(pane).is_none() {
                return false;
            }
            pending.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }

    /// Epoch-guarded jobs-map removal for cancel paths that KEEP the
    /// intent (`cancel_job_only_for`): remove only while the map holds
    /// this Arc at the entry epoch. Holds the pending lock across the
    /// check (read-only) so a racing `publish_submit` — which publishes
    /// under the pending lock — cannot slip between check and remove.
    pub async fn remove_job_if_epoch(
        &self,
        pane: &str,
        job: &Arc<crate::jobs::job::Job>,
        epoch_at_entry: u64,
    ) -> bool {
        let mut jobs = self.jobs.lock().await;
        let _pending = self.pending.lock().await;
        if job.epoch.load(std::sync::atomic::Ordering::Relaxed) != epoch_at_entry {
            return false;
        }
        if jobs.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
            jobs.remove(pane);
            true
        } else {
            false
        }
    }
}
