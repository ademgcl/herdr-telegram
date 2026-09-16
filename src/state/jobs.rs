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
        let removed = {
            let mut map = self.jobs.lock().await;
            if map.get(pane).map(|j| Arc::ptr_eq(j, &job)).unwrap_or(false) {
                map.remove(pane);
                true
            } else {
                false
            }
        };
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

    /// Record a submitted prompt durably (cleared on settle/cancel).
    /// Disk write happens AFTER the guard drops (never hold `pending`
    /// across serde + blocking fs — it blocks every intent user).
    pub async fn remember_pending(&self, pane: &str, chat: i64, thread: Option<i64>, prompt: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let snap = {
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
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
    }

    pub async fn clear_pending(&self, pane: &str) {
        let snap = {
            let mut map = self.pending.lock().await;
            if map.remove(pane).is_none() {
                return;
            }
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
    }

    pub async fn clear_all_pending(&self) {
        let empty = {
            let mut map = self.pending.lock().await;
            if map.is_empty() {
                return;
            }
            map.clear();
            map.clone()
        };
        persist::save_file(&persist::store_path(), &empty);
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

    /// Drop reply targets pointing at a dead pane (lock order torder →
    /// targets, as in `remember`): replying to a stale card must say
    /// "who?", never re-arm focus on the corpse into a void loop.
    pub(crate) async fn clear_targets_for(&self, pane: &str) {
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        let dead: Vec<(i64, i64)> = map
            .iter()
            .filter(|(_, p)| p.as_str() == pane)
            .map(|(k, _)| *k)
            .collect();
        for k in &dead {
            map.remove(k);
        }
        ord.retain(|o| !dead.contains(o));
    }

    /// F11: Start sustaining a "typing…" action in the pane's topic while working.
    pub async fn start_typing(self: &Arc<Self>, pane: &str) {
        // DM mode has no forum: nothing to type into, and the spawned
        // task would exit instantly while leaking its handle.
        if self.cfg.forum.is_none() {
            return;
        }
        let mut tasks = self.typing_tasks.lock().await;
        // Reap dead handles: a panicked task must not block its
        // replacement forever.
        tasks.retain(|_, h| !h.is_finished());
        if tasks.contains_key(pane) {
            return;
        }
        let s = self.clone();
        let pane_str = pane.to_string();
        let handle = tokio::spawn(async move {
            let forum = s.cfg.forum;
            // General-origin jobs never have a thread; topic jobs use
            // theirs — and go quiet (not General-spammy) while a lost
            // mapping heals instead of typing into the parent chat.
            // Bounded quiet: after ~12s without a thread, nudge General
            // once per 3 ticks so a mid-job delete never looks dead.
            let mut had_thread = false;
            let mut quiet_ticks: u32 = 0;
            while let Some(chat_id) = forum {
                let thread = s.topics.all_mappings().get(&pane_str).copied();
                if let Some(th) = thread {
                    had_thread = true;
                    quiet_ticks = 0;
                    s.tg.typing(chat_id, Some(th)).await;
                } else if !had_thread {
                    s.tg.typing(chat_id, None).await;
                } else {
                    quiet_ticks += 1;
                    if quiet_ticks.is_multiple_of(3) {
                        eprintln!("[typing] {pane_str} mapping lost — nudging General");
                        s.tg.typing(chat_id, None).await;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }
        });
        tasks.insert(pane.to_string(), handle);
    }

    /// Stop sustaining the "typing…" action for this pane.
    /// Unconditional primitive (shutdown paths); live paths prefer
    /// `stop_typing_unless_owned` so a successor keeps its task.
    #[allow(dead_code)]
    pub async fn stop_typing(&self, pane: &str) {
        if let Some(handle) = self.typing_tasks.lock().await.remove(pane) {
            handle.abort();
        }
    }

    /// Stop only when no job owns the pane. Two separate locks (never
    /// nested — nesting `typing_tasks`→`jobs` once deadlocked against a
    /// future inverse): check-then-act TOCTOU is self-healing — if a
    /// successor inserted after our check, we re-mint its task after the
    /// abort so the kill is only a blip. Producers must insert into `jobs`
    /// BEFORE spawn → `start_typing` or a mint-after-check still races.
    pub async fn stop_typing_unless_owned(self: &Arc<Self>, pane: &str) {
        if self.jobs.lock().await.contains_key(pane) {
            return;
        }
        let aborted = if let Some(handle) = self.typing_tasks.lock().await.remove(pane) {
            handle.abort();
            true
        } else {
            false
        };
        // Successor won the race after our check: re-mint so the abort
        // above is a blip, not a dark pane.
        if aborted && self.jobs.lock().await.contains_key(pane) {
            self.start_typing(pane).await;
        }
    }
}
