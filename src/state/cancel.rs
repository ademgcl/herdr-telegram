//! Watcher retire paths for shared state. Split from `state` (300-line
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
        // Entry pins for the LWW guards below: a submit racing the
        // snapshot owns the slot (same-Arc reuse bumps the epoch).
        let epoch_at_entry = cur.as_ref().map(|j| j.epoch.load(Ordering::Relaxed));
        let pending_at_entry = self.pending.lock().await.get(pane).cloned();
        // Typing: shell-aware ownership check (a live agent successor
        // or a pending shell keeps its task; an orphan stops here —
        // the Some branch relies on this for the no-job case too).
        self.stop_shell_typing(pane).await;
        let Some(job) = cur else {
            // No job: a successor inserted after still wins — leave it
            // alone; otherwise clear orphan/shell intent (suppresses cards).
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            // LWW: only clear the intent we actually saw — a submit
            // racing the snapshot owns the changed slot.
            let had_pending = self
                .clear_pending_if_unchanged(pane, pending_at_entry.clone())
                .await;
            if !had_pending
                && self.pending.lock().await.get(pane) != pending_at_entry.as_ref()
            {
                // Racing submit owns the changed slot — its debounce and
                // done stamp belong to the new turn (missed-buzz guard).
                return false;
            }
            // No job: disarm waiters/debounce/episode (stale arms stay dead).
            self.clear_waiters(pane).await;
            self.clear_limit_episode(pane).await;
            self.debounce.lock().await.remove(pane);
            // In-flight DM spontaneous owns no arm to disarm (missing arm
            // proceeds by design) — stamp done so its inside-post check
            // aborts via done-after instead of posting past the /cancel.
            self.last_done
                .lock()
                .await
                .insert(pane.to_string(), std::time::Instant::now());
            return had_pending;
        };
        // Epoch-guarded atomic retire (map entry + intent): a submit
        // racing the snapshot owns the pane on bump (same Arc or new).
        if !self
            .cancel_retire_if_epoch(pane, &job, epoch_at_entry.unwrap_or(0))
            .await
        {
            // A successor won the race: leave its intent/waiters alone.
            return false;
        }
        self.clear_waiters(pane).await;
        // Fresh stall episode after cancel (no 30-min inherit).
        self.clear_limit_episode(pane).await;
        // Disarm a pending settle debounce (armed card must not land after).
        self.debounce.lock().await.remove(pane);
        // Same DM done-stamp as the no-job branch above: the in-flight
        // screen read already passed the job-live check.
        self.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
        // After removal the pane is unowned, so this actually stops the
        // task now (the pre-removal stop above is a no-op while the job
        // is present — without this the indicator lingers on the
        // watcher's footer, up to a 60s backoff away). Shell-aware: a
        // racing shell submit's pending keeps its task.
        self.stop_shell_typing(pane).await;
        job.mark_stopped();
        // Bump the epoch: an in-flight finalize aborts at its next checkpoint
        // instead of posting into a cancelled world.
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
        let epoch_at_entry = cur.as_ref().map(|j| j.epoch.load(Ordering::Relaxed));
        let pending_at_entry = self.pending.lock().await.get(pane).cloned();
        let Some(job) = cur else {
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            // LWW (loud parity): a submit racing the snapshot owns it.
            // A changed slot means a live successor — keep its debounce
            // (dropping it loses the next settle's card).
            if !self
                .clear_pending_if_unchanged(pane, pending_at_entry.clone())
                .await
                && self.pending.lock().await.get(pane) != pending_at_entry.as_ref()
            {
                return false;
            }
            self.clear_waiters(pane).await;
            self.clear_limit_episode(pane).await;
            self.debounce.lock().await.remove(pane);
            // No watcher left to reap the typing task — stop it here
            // (the job path below relies on the watcher's own footer).
            // Shell-aware: a concurrent shell submit keeps its task.
            self.stop_shell_typing(pane).await;
            return false;
        };
        // Epoch-guarded atomic retire (loud parity): a racing submit
        // keeps its entry + intent.
        if !self
            .cancel_retire_if_epoch(pane, &job, epoch_at_entry.unwrap_or(0))
            .await
        {
            return false;
        }
        self.clear_waiters(pane).await;
        // Fresh stall episode after quiet retire (loud parity): a
        // same-name remint must not inherit limit_alert/seen/miss/cool.
        self.clear_limit_episode(pane).await;
        self.debounce.lock().await.remove(pane);
        // After removal: unowned, so this actually stops the task
        // (shell-aware like the loud path — a racing shell submit keeps
        // its task via its pending).
        self.stop_shell_typing(pane).await;
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
        let epoch_at_entry = cur.as_ref().map(|j| j.epoch.load(Ordering::Relaxed));
        let Some(job) = cur else {
            // Lost-race guard (loud/quiet parity): a successor inserted
            // after the snapshot owns the fresh debounce — leave it alone.
            // Checked just before the remove: a job landing in between
            // still owns it (sequential checks, never nested locks).
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            self.debounce.lock().await.remove(pane);
            return false;
        };
        // Epoch-guarded remove (intent preserved): a racing submit keeps
        // its entry — a same-Arc bump is invisible to ptr_eq alone.
        if !self
            .remove_job_if_epoch(pane, &job, epoch_at_entry.unwrap_or(0))
            .await
        {
            return false;
        }
        self.debounce.lock().await.remove(pane);
        self.stop_shell_typing(pane).await;
        job.mark_stopped();
        job.epoch.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub async fn cancel_all_jobs(&self) -> usize {
        // Narrow critical sections: take each map, drop its guard, then
        // act — never hold typing_tasks across the jobs/pending/waiter
        // locks (a future inverse nesting would deadlock, and every
        // typing start/stop blocks for the whole global cancel).
        // 1:1 working↔typing: abort only panes that owned cancellable
        // work (job or intent) — a global /cancel must not darken a
        // spontaneous working bystander with neither (its next heal is
        // otherwise the 60s watchdog while it keeps working).
        let job_panes: std::collections::HashSet<String> =
            self.jobs.lock().await.keys().cloned().collect();
        let pending_panes: std::collections::HashSet<String> =
            self.pending.lock().await.keys().cloned().collect();
        let doomed: Vec<tokio::task::JoinHandle<()>> = {
            let mut tasks = self.typing_tasks.lock().await;
            let kill: Vec<String> = tasks
                .keys()
                .filter(|p| job_panes.contains(*p) || pending_panes.contains(*p))
                .cloned()
                .collect();
            kill.into_iter().filter_map(|p| tasks.remove(&p)).collect()
        };
        for handle in doomed {
            handle.abort();
        }
        // Last-writer-wins (per-pane parity): an enqueue landing between
        // the snapshots above and the clears below owns its intent — a
        // blind take-all + clear-all would wipe a successfully submitted
        // prompt with no reply ever arriving. Same-Arc reuse bumps the
        // epoch in place, so the snapshot pairs each Arc with its epoch.
        let live_jobs: HashMap<String, (Arc<Job>, u64)> = {
            let map = self.jobs.lock().await;
            map.iter()
                .map(|(p, j)| (p.clone(), (j.clone(), j.epoch.load(Ordering::Relaxed))))
                .collect()
        };
        let live_pending = self.pending.lock().await.clone();
        let jobs: HashMap<String, Arc<Job>> = {
            let mut map = self.jobs.lock().await;
            let mut taken = HashMap::new();
            for (p, (j, epoch)) in &live_jobs {
                let same = map
                    .get(p)
                    .map(|c| Arc::ptr_eq(c, j) && c.epoch.load(Ordering::Relaxed) == *epoch)
                    .unwrap_or(false);
                if same && let Some(removed) = map.remove(p) {
                    taken.insert(p.clone(), removed);
                }
            }
            taken
        };
        {
            let mut map = self.pending.lock().await;
            let mut snap = live_pending.clone();
            snap.retain(|p, pp| {
                map.get(p)
                    .map(|c| c.chat == pp.chat && c.thread == pp.thread && c.prompt == pp.prompt)
                    .unwrap_or(false)
            });
            for p in snap.keys() {
                map.remove(p);
            }
            let remaining = map.clone();
            drop(map);
            crate::jobs::persist::save_file(&crate::jobs::persist::store_path(), &remaining);
        }
        // Global cancel retires everything: armed input waiters and
        // settle debounces die with the jobs, or the next message/card
        // would serve a cancelled world.
        self.typewait.lock().await.clear();
        self.keywait.lock().await.clear();
        self.runwait.lock().await.clear();
        self.debounce.lock().await.clear();
        // Fresh episodes everywhere after a global cancel (see
        // cancel_jobs_for for the per-pane reason).
        self.clear_all_limit_episodes().await;
        // DM done-stamps (cancel_jobs_for parity): in-flight DM
        // spontaneous owns no arm — without this it posts past the
        // global cancel via its done-after check.
        {
            let now = std::time::Instant::now();
            let mut done = self.last_done.lock().await;
            for pane in job_panes.union(&pending_panes) {
                done.insert(pane.clone(), now);
            }
        }
        let count = jobs.len();
        for job in jobs.values() {
            job.mark_stopped();
            // Same epoch-bump as cancel_jobs_for: in-flight posts abort.
            job.epoch.fetch_add(1, Ordering::Relaxed);
            job.cancel.notify_waiters();
        }
        count
    }
}

/// Re-exported test helper (split to `test_state`, 300-line file
/// limit): existing `state::cancel::isolated_state` call sites keep
/// working unchanged.
#[cfg(test)]
pub(crate) use super::test_state::isolated_state;

#[cfg(test)]
#[path = "cancel_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "cancel_race_tests.rs"]
mod race_tests;
