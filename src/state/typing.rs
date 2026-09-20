//! Typing-indicator task ownership. Split from `jobs` (300-line file limit).
use super::State;
use std::sync::Arc;

impl State {
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
            let Some(chat_id) = s.cfg.forum else {
                return;
            };
            // 1:1 working↔typing: fire-and-forget per tick — an awaited
            // sendChatAction under a slow Telegram stretches the cycle
            // past the ~5s expiry and the indicator flickers. Overlaps
            // are idempotent refreshes. Pause while unmapped (mid-job
            // delete healing): a topic indicator typed into General
            // shows nowhere useful and misleads.
            loop {
                if let Some(th) = s.topics.all_mappings().get(&pane_str).copied() {
                    let tg = s.tg.clone();
                    tokio::spawn(async move {
                        tg.typing(chat_id, Some(th)).await;
                    });
                }
                tokio::time::sleep(std::time::Duration::from_secs(super::TYPING_TICK_SECS)).await;
            }
        });
        tasks.insert(pane.to_string(), handle);
    }

    /// Shell-aware stop: shells own `pending`, agents own `jobs`, and
    /// both share one per-pane typing task. Plain `stop_typing_unless_owned`
    /// checks only `jobs`, so a finished/superseded shell settle would
    /// abort the task a still-running successor (or overlapping agent)
    /// needs. Stop only when neither owns the pane; re-mint on race.
    pub async fn stop_shell_typing(self: &Arc<Self>, pane: &str) {
        // Live-only: a stopped corpse between mark_stopped() and map
        // removal must not keep the typing task alive past retire.
        if self.job_live(pane).await {
            return;
        }
        if self.pending.lock().await.contains_key(pane) {
            return;
        }
        let aborted = if let Some(handle) = self.typing_tasks.lock().await.remove(pane) {
            handle.abort();
            true
        } else {
            false
        };
        if aborted && (self.job_live(pane).await || self.pending.lock().await.contains_key(pane)) {
            self.start_typing(pane).await;
        }
    }

    /// Stop only when no job owns the pane. Two separate locks (never
    /// nested — nesting `typing_tasks`→`jobs` once deadlocked against a
    /// future inverse): check-then-act TOCTOU is self-healing — if a
    /// successor inserted after our check, we re-mint its task after the
    /// abort so the kill is only a blip. Producers must insert into `jobs`
    /// BEFORE spawn → `start_typing` or a mint-after-check still races.
    pub async fn stop_typing_unless_owned(self: &Arc<Self>, pane: &str) {
        if self.job_live(pane).await {
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
        if aborted && self.job_live(pane).await {
            self.start_typing(pane).await;
        }
    }
}
