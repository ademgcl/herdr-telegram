//! Job + waiter lifecycle for shared state. Split from `state` (300-line
//! file limit): prompt-intent durability, watcher retire, and per-pane
//! cleanup live here as `impl State`.
use super::State;
use crate::jobs::{
    job::Job,
    persist::{self, PendingPrompt},
};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

impl State {
    /// Retire one pane's watcher (used by /quit: no agent left to watch).
    pub async fn cancel_jobs_for(&self, pane: &str) -> bool {
        let job = self.jobs.lock().await.remove(pane);
        self.clear_pending(pane).await;
        self.clear_waiters(pane).await;
        // Disarm a pending settle debounce: without this a card armed
        // before the cancel lands after the confirmation.
        self.debounce.lock().await.remove(pane);
        match job {
            Some(job) => {
                job.mark_stopped();
                // Bump the epoch: an in-flight finalize aborts at its next
                // checkpoint instead of posting into a cancelled world.
                job.epoch.fetch_add(1, Ordering::Relaxed);
                job.cancel.notify_waiters();
                true
            }
            None => false,
        }
    }

    pub async fn cancel_all_jobs(&self) -> usize {
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

    /// Record a submitted prompt durably (cleared on settle/cancel).
    pub async fn remember_pending(&self, pane: &str, chat: i64, thread: Option<i64>, prompt: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut map = self.pending.lock().await;
        map.insert(
            pane.to_string(),
            PendingPrompt {
                chat,
                thread,
                prompt: prompt.to_string(),
                started_unix: now,
            },
        );
        persist::save_file(&persist::store_path(), &map);
    }

    pub async fn clear_pending(&self, pane: &str) {
        let mut map = self.pending.lock().await;
        if map.remove(pane).is_some() {
            persist::save_file(&persist::store_path(), &map);
        }
    }

    pub async fn clear_all_pending(&self) {
        let mut map = self.pending.lock().await;
        if !map.is_empty() {
            map.clear();
            persist::save_file(&persist::store_path(), &map);
        }
    }

    /// End one pane's stall episode (limit alert, stuck timer, absence
    /// streak) — called when the pane leaves `working`, so the next stall
    /// re-alerts fresh instead of inheriting the prior episode's dedup.
    pub async fn clear_limit_episode(&self, pane: &str) {
        self.limit_alert.lock().await.remove(pane);
        self.limit_seen.lock().await.remove(pane);
        self.limit_miss.lock().await.remove(pane);
    }

    /// Drop armed input waiters for a dead pane: a typewait surviving
    /// /kill would eat the owner's next message as typed input into a
    /// pane that no longer exists.
    pub(crate) async fn clear_waiters(&self, pane: &str) {
        self.typewait.lock().await.retain(|_, p| p != pane);
        self.keywait.lock().await.retain(|_, p| p != pane);
        self.runwait.lock().await.retain(|_, p| p != pane);
    }
}
