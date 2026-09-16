//! Job + waiter lifecycle for shared state. Split from `state` (300-line
//! file limit): prompt-intent durability, waiter retire, and per-pane
//! cleanup live here as `impl State` (cancel paths live in `cancel`).
use super::State;
use crate::jobs::persist::{self, PendingPrompt};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

impl State {
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

    /// Scoped /cancel for General/DM (topic /cancel already names its
    /// pane): `all` nukes everything explicitly, an explicit pane id
    /// cancels one, bare follows focus — a bare global nuke with no
    /// warning wiped every recovered/running prompt's intent. Callers
    /// clear their own chat waiters first; returns the ack text.
    pub async fn cancel_scoped(self: &Arc<Self>, pane_arg: &str) -> String {
        // First word only (`/cancel w1:p1 extra` still names the pane);
        // `all` is case-insensitive. Hash-only (`#`) is a typo, never
        // focus: fall through to the no-job message with the raw echo.
        let raw = pane_arg.split_whitespace().next().unwrap_or("").trim();
        let arg = raw.trim_start_matches('#');
        let running: Vec<String> = {
            let mut v: Vec<String> = self.jobs.lock().await.keys().cloned().collect();
            for p in self.pending.lock().await.keys() {
                if !v.contains(p) {
                    v.push(p.clone());
                }
            }
            v.sort();
            v
        };
        if arg.eq_ignore_ascii_case("all") {
            let n = running.len();
            self.cancel_all_jobs().await;
            return format!("✋ cancelled {n} pending job(s) (all panes)");
        }
        let list = if running.is_empty() {
            "none".to_string()
        } else {
            running.join(", ")
        };
        let target: Option<String> = if !arg.is_empty() {
            Some(arg.to_string())
        } else if !raw.is_empty() {
            // Hash-only typo: surface it instead of cancelling focus.
            Some(raw.to_string())
        } else {
            self.get_focus().await
        };
        match target {
            Some(p) if running.contains(&p) => {
                self.cancel_jobs_for(&p).await;
                format!("✋ cancelled {p}")
            }
            Some(p) => format!("no job for {p} — running: {list}"),
            None => format!("nothing focused — running: {list} — /cancel <pane> or /cancel all"),
        }
    }

    /// True when the durable intent still belongs to this exact submit.
    /// One slot per pane is shared by agent prompts and shell commands —
    /// last-writer-wins by overwrite — so waiters must only serve (and
    /// clear) their own: an overlapping submit or an agent re-entry must
    /// neither be served another command's output nor wipe its intent.
    pub async fn pending_matches(
        &self,
        pane: &str,
        chat: i64,
        thread: Option<i64>,
        prompt: &str,
    ) -> bool {
        self.pending
            .lock()
            .await
            .get(pane)
            .map(|p| p.chat == chat && p.thread == thread && p.prompt == prompt)
            .unwrap_or(false)
    }

    /// End one pane's stall episode (limit alert, stuck timer, absence
    /// streak, send cooldown) — called on shell flips and pane death, or
    /// after confirmed-clean reads. Working flicker deliberately does NOT
    /// clear: opencode retries quota stalls across working↔idle samples,
    /// and wiping per flicker kept genuine stalls silent for hours.
    pub async fn clear_limit_episode(&self, pane: &str) {
        self.limit_alert.lock().await.remove(pane);
        self.limit_seen.lock().await.remove(pane);
        self.limit_miss.lock().await.remove(pane);
        self.limit_send_cool.lock().await.remove(pane);
    }

    /// End ALL stall episodes (global /cancel starts every pane fresh).
    pub async fn clear_all_limit_episodes(&self) {
        self.limit_alert.lock().await.clear();
        self.limit_seen.lock().await.clear();
        self.limit_miss.lock().await.clear();
        self.limit_send_cool.lock().await.clear();
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
            // theirs. 1:1 working↔typing: while unmapped (mid-job delete
            // healing), type into General EVERY 4s tick — a ~5s-expiry
            // indicator must never see a gap (an earlier once-per-3-ticks
            // throttle left exactly such a dropout mid-job).
            while let Some(chat_id) = forum {
                let thread = s.topics.all_mappings().get(&pane_str).copied();
                if let Some(th) = thread {
                    s.tg.typing(chat_id, Some(th)).await;
                } else {
                    s.tg.typing(chat_id, None).await;
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

    /// Shell-aware stop: shells own `pending`, agents own `jobs`, and
    /// both share one per-pane typing task. Plain `stop_typing_unless_owned`
    /// checks only `jobs`, so a finished/superseded shell settle would
    /// abort the task a still-running successor (or overlapping agent)
    /// needs. Stop only when neither owns the pane; re-mint on race.
    pub async fn stop_shell_typing(self: &Arc<Self>, pane: &str) {
        if self.jobs.lock().await.contains_key(pane) {
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
        if aborted
            && (self.jobs.lock().await.contains_key(pane)
                || self.pending.lock().await.contains_key(pane))
        {
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
