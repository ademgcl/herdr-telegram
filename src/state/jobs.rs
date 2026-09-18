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

    /// Returns true when a pending intent was actually removed.
    pub async fn clear_pending(&self, pane: &str) -> bool {
        let snap = {
            let mut map = self.pending.lock().await;
            if map.remove(pane).is_none() {
                return false;
            }
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }

    /// Clear only when the slot still holds this exact submit (same
    /// TOCTOU as `remember_pending_cas`): a resubmit landing between a
    /// `pending_matches` check and the clear (a `send_msg` await sits
    /// between them) must not lose its intent to the loser's clear.
    pub async fn clear_pending_if_matches(
        &self,
        pane: &str,
        chat: i64,
        thread: Option<i64>,
        prompt: &str,
    ) -> bool {
        let snap = {
            let mut map = self.pending.lock().await;
            let mine = map
                .get(pane)
                .map(|p| p.chat == chat && p.thread == thread && p.prompt == prompt)
                .unwrap_or(false);
            if !mine {
                return false;
            }
            map.remove(pane);
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
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

    /// Atomic check-and-remember for race-prone restores (remap
    /// migration, reconcile owed-intent restores): under ONE `pending`
    /// guard with no await inside, remember `set` only when the slot is
    /// vacant or still holds the `check` triple. A separate check-then-
    /// remember across awaits lets a submit landing between them be
    /// overwritten by corpse text (lost reply) — and holding the guard
    /// across `pending_matches` deadlocks (non-reentrant tokio Mutex).
    /// A restore keeps the ORIGINAL timestamp (never re-stamps now):
    /// fresh stamps on every restore would defeat the 24h stale bound
    /// and keep corpse intents immortal. Timestamp + clone inside the
    /// guard (no await), disk write after (never hold `pending` across
    /// serde + blocking fs). True when anything was written.
    pub async fn remember_pending_cas(
        &self,
        pane: &str,
        check: (i64, Option<i64>, &str),
        set: (i64, Option<i64>, &str),
    ) -> bool {
        let snap = {
            let mut map = self.pending.lock().await;
            let started_unix = match map.get(pane) {
                None => SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                Some(p)
                    if p.chat == check.0 && p.thread == check.1 && p.prompt == check.2 =>
                {
                    p.started_unix
                }
                _ => return false,
            };
            map.insert(
                pane.to_string(),
                PendingPrompt {
                    chat: set.0,
                    thread: set.1,
                    prompt: set.2.to_string(),
                    started_unix,
                },
            );
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }

    /// Bump the shell submit generation for `pane` (one submit = one
    /// generation). Settles snapshot the return and serve only it: an
    /// identical re-command shares the pending slot's text, so text
    /// equality alone would let the old settle post and clear the
    /// newcomer's intent (silent reply loss).
    pub async fn bump_shell_epoch(&self, pane: &str) -> u64 {
        let mut map = self.shell_gen.lock().await;
        let n = map.get(pane).copied().unwrap_or(0).wrapping_add(1);
        map.insert(pane.to_string(), n);
        n
    }

    /// True when `epoch` is still the pane's latest submit (no resubmit
    /// landed since the snapshot). Check alongside every
    /// `pending_matches` gate in the shell settle.
    pub async fn shell_epoch_is(&self, pane: &str, epoch: u64) -> bool {
        self.shell_gen.lock().await.get(pane).copied() == Some(epoch)
    }

    /// Clear the shell intent only for our own generation (same TOCTOU
    /// as `clear_pending_if_matches`, plus the generation): a resubmit
    /// racing the send owns the slot now, even byte-identical text.
    pub async fn clear_shell_if_matches(
        &self,
        pane: &str,
        chat: i64,
        thread: Option<i64>,
        prompt: &str,
        epoch: u64,
    ) -> bool {
        if !self.shell_epoch_is(pane, epoch).await {
            return false;
        }
        self.clear_pending_if_matches(pane, chat, thread, prompt).await
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
        self.typewait.lock().await.retain(|_, (p, _)| p != pane);
        self.keywait.lock().await.retain(|_, (p, _)| p != pane);
        // No runwait line: values are workspace ids (never panes) and
        // expiry lives in hygiene — nothing pane-bound to drop here.
    }
}
