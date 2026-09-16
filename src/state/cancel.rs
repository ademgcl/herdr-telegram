//! Watcher retire paths for shared state. Split from `jobs` (300-line
//! file limit): loud user-cancel, quiet pane-death retire, global cancel.
//! All single-pane retires are last-writer-wins (snapshot + remove-if-same
//! Arc) and never nest async locks.
use super::State;
use crate::jobs::job::Job;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};

impl State {
    /// Retire one pane's watcher. Last-writer-wins: snapshot the job,
    /// then remove only if the map still holds that same Arc — a prompt
    /// inserted after the snapshot survives (new work beats a racing
    /// /cancel), a prompt before it is cancelled. All callers (dead-pane
    /// reap + live /cancel) share this. Typing uses ownership-check so a
    /// live successor keeps its task.
    pub async fn cancel_jobs_for(self: &Arc<Self>, pane: &str) -> bool {
        let cur = self.jobs.lock().await.get(pane).cloned();
        // Typing: ownership-checked (a live successor keeps its task).
        self.stop_typing_unless_owned(pane).await;
        let Some(job) = cur else {
            // No job at snapshot: a successor inserted after still wins —
            // leave it alone. Otherwise clear orphan/shell intent so a
            // /cancel suppresses a settling shell card.
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            self.clear_pending(pane).await;
            // No job: still disarm debounce/waiters for the pane (a stale
            // arm must not fire after the confirmation).
            self.clear_waiters(pane).await;
            self.debounce.lock().await.remove(pane);
            return false;
        };
        let removed = Self::remove_if_same(&self.jobs, pane, &job).await;
        if !removed {
            // A successor won the race: leave its intent/waiters alone.
            return false;
        }
        self.clear_pending(pane).await;
        self.clear_waiters(pane).await;
        // Disarm a pending settle debounce: without this a card armed
        // before the cancel lands after the confirmation.
        self.debounce.lock().await.remove(pane);
        job.mark_stopped();
        // Bump the epoch: an in-flight finalize aborts at its next
        // checkpoint instead of posting into a cancelled world.
        job.epoch.fetch_add(1, Ordering::Relaxed);
        job.cancel.notify_waiters();
        true
    }

    /// Quiet retire for pane-death paths (reconcile dead/shell flip):
    /// same removal + books as `cancel_jobs_for` but WITHOUT the cancel
    /// notify, so a parked watcher exits silently at its next wake
    /// instead of posting a "✋ cancelled" card next to the death
    /// notice. Typing stops immediately: removal happens first, so the
    /// ownership check passes (the loud variant stops before removal and
    /// relies on the watcher's own footer).
    pub async fn cancel_jobs_for_quiet(self: &Arc<Self>, pane: &str) -> bool {
        let cur = self.jobs.lock().await.get(pane).cloned();
        let Some(job) = cur else {
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            self.clear_pending(pane).await;
            self.clear_waiters(pane).await;
            self.debounce.lock().await.remove(pane);
            // No watcher left to reap the typing task — stop it here
            // (the job path below relies on the watcher's own footer).
            self.stop_typing_unless_owned(pane).await;
            return false;
        };
        if !Self::remove_if_same(&self.jobs, pane, &job).await {
            return false;
        }
        self.clear_pending(pane).await;
        self.clear_waiters(pane).await;
        self.debounce.lock().await.remove(pane);
        // After removal: unowned, so this actually stops the task.
        self.stop_typing_unless_owned(pane).await;
        job.mark_stopped();
        job.epoch.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Retire only the watcher job, preserving pending intent + waiters.
    /// Reconcile's already-shell branch uses this: a stale watcher on a
    /// live shell pane must die without eating an active shell command's
    /// pending intent (same last-writer-wins remove-if-same as above).
    /// No cancel notify: the parked watcher exits silently via
    /// `is_stopped` instead of posting a spurious "✋ cancelled" card
    /// into the shell topic. Stale settle debounce is cleared (it would
    /// else fire an agent-spontaneous card into the shell topic); typing
    /// stops now that the entry is gone (a successor keeps its task via
    /// the ownership check, same as the quiet path).
    pub async fn cancel_job_only_for(self: &Arc<Self>, pane: &str) -> bool {
        let cur = self.jobs.lock().await.get(pane).cloned();
        let Some(job) = cur else {
            self.debounce.lock().await.remove(pane);
            return false;
        };
        if !Self::remove_if_same(&self.jobs, pane, &job).await {
            return false;
        }
        self.debounce.lock().await.remove(pane);
        self.stop_typing_unless_owned(pane).await;
        job.mark_stopped();
        job.epoch.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub async fn cancel_all_jobs(&self) -> usize {
        // Narrow critical sections: take each map, drop its guard, then
        // act — never hold typing_tasks across the jobs/pending/waiter
        // locks (a future inverse nesting would deadlock, and every
        // typing start/stop blocks for the whole global cancel).
        let typing: Vec<tokio::task::JoinHandle<()>> =
            std::mem::take(&mut *self.typing_tasks.lock().await)
                .into_values()
                .collect();
        for handle in typing {
            handle.abort();
        }
        let jobs: HashMap<String, Arc<Job>> = std::mem::take(&mut *self.jobs.lock().await);
        self.clear_all_pending().await;
        // Global cancel retires everything: armed input waiters and
        // settle debounces die with the jobs, or the next message/card
        // would serve a cancelled world.
        self.typewait.lock().await.clear();
        self.keywait.lock().await.clear();
        self.runwait.lock().await.clear();
        self.debounce.lock().await.clear();
        let count = jobs.len();
        for job in jobs.values() {
            job.mark_stopped();
            // Same epoch-bump as cancel_jobs_for: in-flight posts abort.
            job.epoch.fetch_add(1, Ordering::Relaxed);
            job.cancel.notify_waiters();
        }
        count
    }

    /// Remove-if-same-Arc: the shared last-writer-wins primitive. Never
    /// holds the guard across anything — check and remove under one lock.
    async fn remove_if_same(
        jobs: &tokio::sync::Mutex<HashMap<String, Arc<Job>>>,
        pane: &str,
        job: &Arc<Job>,
    ) -> bool {
        let mut map = jobs.lock().await;
        if map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
            map.remove(pane);
            true
        } else {
            false
        }
    }
}
