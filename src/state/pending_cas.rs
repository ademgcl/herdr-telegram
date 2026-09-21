//! Atomic pending-intent guards for shared state. Split from `jobs`
//! (300-line file limit): match checks + check-and-remember restores
//! live here as `impl State`.
use super::State;
use crate::jobs::persist::{self, PendingPrompt};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
#[path = "pending_cas_tests.rs"]
mod tests;

impl State {
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

    /// Stamp-pinned match (single source for reconcile's post-RPC
    /// re-validations): the triple alone can't tell an identical
    /// re-prompt (same chat/thread/text, fresh `started_unix`) from the
    /// owed turn — a submit racing the tail/close RPCs would else match
    /// and post a stale quit card beside live work (or retire fresh
    /// work as the corpse).
    pub async fn pending_matches_stamp(
        &self,
        pane: &str,
        chat: i64,
        thread: Option<i64>,
        prompt: &str,
        started_unix: u64,
    ) -> bool {
        self.pending.lock().await.get(pane).map(|p| {
            p.chat == chat
                && p.thread == thread
                && p.prompt == prompt
                && p.started_unix == started_unix
        }).unwrap_or(false)
    }

    /// Atomic check-and-remember for race-prone restores (reconcile
    /// owed-intent restores): under ONE `pending` guard with no await
    /// inside, remember `set` only when the slot is vacant or still holds
    /// the `check` triple (a separate check-then-remember across awaits
    /// lets a submit landing between them be overwritten by corpse text;
    /// holding the guard across `pending_matches` deadlocks). A restore
    /// passes the ORIGINAL `started_unix`: the vacant-slot branch stamps
    /// now, which would defeat the 24h stale bound and keep a corpse
    /// intent immortal (reconcile's failed-notice restore).
    pub async fn remember_pending_cas_with_time(
        &self,
        pane: &str,
        check: (i64, Option<i64>, &str),
        set: (i64, Option<i64>, &str),
        started_unix: Option<u64>,
    ) -> bool {
        self.remember_pending_cas_inner(pane, check, set, started_unix, true)
            .await
    }

    /// Migration-only strict variant (repoint): occupied-match only, never
    /// vacant-insert. A vacant slot means cancelled/settled — minting the
    /// corpse text there with a fresh stamp resurrects dead work with a
    /// fresh 24h clock. Close/vanish restores keep the lenient entry
    /// above (vacant slots there are owed retries, not corpses).
    pub async fn migrate_pending_cas(
        &self,
        pane: &str,
        check: (i64, Option<i64>, &str),
        set: (i64, Option<i64>, &str),
    ) -> bool {
        self.remember_pending_cas_inner(pane, check, set, None, false)
            .await
    }

    async fn remember_pending_cas_inner(
        &self,
        pane: &str,
        check: (i64, Option<i64>, &str),
        set: (i64, Option<i64>, &str),
        started_unix: Option<u64>,
        allow_vacant: bool,
    ) -> bool {
        let now = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        };
        let snap = {
            let mut map = self.pending.lock().await;
            let started = match map.get(pane) {
                None if allow_vacant => started_unix.unwrap_or_else(now),
                None => return false,
                // Occupied-match keeps the LIVE slot's stamp: the passed
                // stamp is the corpse's, and an identical re-prompt racing
                // the close/vanish RPCs holds a fresh stamp here —
                // regressing it to the corpse's would age the fresh intent
                // toward the 24h stale drop. The passed stamp applies to
                // the vacant branch only.
                Some(p) if p.chat == check.0 && p.thread == check.1 && p.prompt == check.2 => {
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
                    started_unix: started,
                },
            );
            map.clone()
        };
        persist::save_file(&persist::store_path(), &snap);
        true
    }
}
