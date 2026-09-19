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
            // A cleared shell pending is a real cancel (callers ack on bool).
            // Single atomic take: no check-then-clear window for a racer.
            let had_pending = self.clear_pending(pane).await;
            // No job: disarm waiters/debounce/episode (stale arms stay dead).
            self.clear_waiters(pane).await;
            self.clear_limit_episode(pane).await;
            self.debounce.lock().await.remove(pane);
            return had_pending;
        };
        let removed = Self::remove_if_same(&self.jobs, pane, &job).await;
        if !removed {
            // A successor won the race: leave its intent/waiters alone.
            return false;
        }
        self.clear_pending(pane).await;
        self.clear_waiters(pane).await;
        // Fresh stall episode after cancel (no 30-min inherit).
        self.clear_limit_episode(pane).await;
        // Disarm a pending settle debounce (armed card must not land after).
        self.debounce.lock().await.remove(pane);
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
        let Some(job) = cur else {
            if self.jobs.lock().await.contains_key(pane) {
                return false;
            }
            self.clear_pending(pane).await;
            self.clear_waiters(pane).await;
            self.debounce.lock().await.remove(pane);
            // No watcher left to reap the typing task — stop it here
            // (the job path below relies on the watcher's own footer).
            // Shell-aware: a concurrent shell submit keeps its task.
            self.stop_shell_typing(pane).await;
            return false;
        };
        if !Self::remove_if_same(&self.jobs, pane, &job).await {
            return false;
        }
        self.clear_pending(pane).await;
        self.clear_waiters(pane).await;
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
        let Some(job) = cur else {
            self.debounce.lock().await.remove(pane);
            return false;
        };
        if !Self::remove_if_same(&self.jobs, pane, &job).await {
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
        let jobs: HashMap<String, Arc<Job>> = std::mem::take(&mut *self.jobs.lock().await);
        self.clear_all_pending().await;
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

/// Isolated AppState for tests (shared by cancel/hygiene/ctl suites).
/// Cancel + reap paths persist jobs.state, so tests must never touch the
/// repo's live files: each call mints a fresh temp state dir. Serialized
/// via a static mutex: `set_var`/`var` is UB under parallel `cargo test`
/// (Edition 2024 marks it unsafe), so holders keep the guard for the
/// whole test (`_dir` alive) and restore the prior value on drop.
#[cfg(test)]
static TEST_ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct TestStateDir {
    path: std::path::PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
    old: Option<std::ffi::OsString>,
}
#[cfg(test)]
impl Drop for TestStateDir {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            unsafe { std::env::set_var("HERDR_STATE_DIR", old) };
        } else {
            unsafe { std::env::remove_var("HERDR_STATE_DIR") };
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
static TEST_DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn isolated_state() -> (crate::state::AppState, TestStateDir) {
    let guard = TEST_ENV_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::var_os("HERDR_STATE_DIR");
    let n = TEST_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("ht-{n}-{nanos}"));
    std::fs::create_dir_all(&path).expect("test tempdir");
    unsafe { std::env::set_var("HERDR_STATE_DIR", &path) };
    let cfg = crate::config::Cfg {
        token: "test-token".to_string(),
        socket: "nonexistent-test.sock".to_string(),
        owners: vec![],
        forum: None,
    };
    let s = super::State::new(cfg).expect("test state");
    (
        s,
        TestStateDir {
            path,
            _guard: guard,
            old,
        },
    )
}

#[cfg(test)]
#[path = "cancel_tests.rs"]
mod tests;
