//! Forum-topic ownership: TopicManager owns Telegram forum-topic CRUD
//! for mapped threads (create/delete/close/reopen/icon/identity
//! card); notifier owns alert CARDS (when/what to buzz), ui owns card
//! TEXT. Mapping state lives in storage; herdr names flow in, never out.
use crate::{
    telegram::TelegramClient,
    topics::{names, storage::TopicStorage},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::Instant,
};

pub struct TopicManager {
    pub(crate) forum_id: Option<i64>,
    pub(crate) socket: Option<String>,
    pub(crate) storage: TopicStorage,
    pub(crate) tg: TelegramClient,
    pub(crate) last_title_write: Mutex<HashMap<String, Instant>>,
    pub(crate) creating: Mutex<HashSet<String>>,
    pub(crate) probe_cursor: Mutex<usize>,
}

/// Single-flight guard: frees the pane's `creating` entry on scope
/// exit (normal, cancel, or panic) so a wedged entry never blocks the
/// pane topic-less forever. Removal is idempotent.
struct CreatingGuard<'a> {
    creating: &'a Mutex<HashSet<String>>,
    pane: String,
}

impl Drop for CreatingGuard<'_> {
    fn drop(&mut self) {
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.pane);
    }
}

mod lifecycle;
mod titles;

pub use lifecycle::ResetNames;

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient, socket: Option<String>) -> Self {
        Self {
            forum_id,
            socket,
            storage: TopicStorage::new(),
            tg,
            last_title_write: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashSet::new()),
            probe_cursor: Mutex::new(0),
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn creating_len(&self) -> usize {
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Single-lock compare-and-delete (storage guard): only drops when
    /// the thread still matches — a stale probe must never delete a
    /// fresh remint. Aux maps cleared only on prune (a remint keeps its
    /// own creating/title-write state).
    pub fn remove_mapping_if_thread(&self, pane: &str, thread: i64) -> bool {
        if !self.storage.remove_if_thread(pane, thread) {
            return false;
        }
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        // Creating is per-pane in-flight: a successful CAS means no live
        // remint holds it (its guard already removed on insert), so
        // clearing a stale leftover is safe and idempotent.
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        true
    }

    pub fn get_pin(&self, pane: &str) -> Option<i64> {
        self.storage.get_pin(pane)
    }

    pub fn set_pin(&self, pane: &str, mid: i64) {
        self.storage.set_pin(pane, mid);
    }

    /// Ensure the pane's topic exists and return its thread. New topics
    /// open under the herdr pane id; the watchdog's 1:1 title sync
    /// renames to the pane label when one is set. Manual renames are
    /// never overwritten blindly — they flow back as pane labels
    /// (`forum_topic_edited` → `pane.rename`), and the stored title
    /// absorbs our own sync echoes.
    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        self.ensure_inner(pane, kind, space, false).await
    }

    /// Reset-owned mint: bypasses the reuse-only gate (Step 4 must
    /// recreate after `clear_all`; the watchdog path stays gated).
    pub async fn ensure_topic_for_reset(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        self.ensure_inner(pane, kind, space, true).await
    }

    async fn ensure_inner(
        &self,
        pane: &str,
        kind: &str,
        space: &str,
        allow_during_reset: bool,
    ) -> Option<i64> {
        let forum = self.forum_id?;
        // A paced reset is rebuilding the map: reuse only, never mint —
        // anything created now is wiped by `clear_all` into an orphan
        // (later re-minted as a double). Reset's own Step 4 bypasses via
        // `allow_during_reset`; deferred panes mint on post-reset ticks.
        if !allow_during_reset && crate::handlers::reset::is_resetting() {
            return self.storage.get_thread(pane);
        }
        // Unknown kind (agent vanished mid-flight): never mint topics —
        // just route to the existing thread, if any.
        if kind == "?" {
            return self.storage.get_thread(pane);
        }
        if let Some(t) = self.storage.get_thread(pane) {
            return Some(t);
        }
        // Single-flight: a concurrent ensure for this same new pane may
        // already be creating. Loser waits up to ~20s (winner budget:
        // 15s timeout × retries + flood-waits); early-None just defers
        // to the next tick, never double-mints.
        if !self
            .creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pane.to_string())
        {
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Some(t) = self.storage.get_thread(pane) {
                    return Some(t);
                }
            }
            return self.storage.get_thread(pane);
        }
        // RAII: task cancel/panic between insert and insert drops the
        // guard instead of wedging the pane topic-less forever.
        let _creating = CreatingGuard {
            creating: &self.creating,
            pane: pane.to_string(),
        };
        // Re-check after claiming single-flight: a reset that started
        // between our first gate and now would wipe this mint into an
        // orphan → later double. Defer to post-reset ticks instead.
        if !allow_during_reset && crate::handlers::reset::is_resetting() {
            return self.storage.get_thread(pane);
        }
        // Tag inside the guard: a cancel before the guard must not leak
        // a persisted tag gap.
        let tag = self.storage.assign_tag(pane, kind);

        let agent = if let Some(sock) = &self.socket {
            crate::herdr::client::get_agent(sock, pane).await.ok()
        } else {
            None
        };
        let raw_title = agent
            .as_ref()
            .map(|a| a.title.as_str())
            .filter(|t| !t.trim().is_empty());
        let branch = agent.as_ref().and_then(|a| a.branch.as_deref());
        let status = agent.as_ref().map(|a| a.status.as_str()).unwrap_or("ready");

        let title_or_tag = raw_title.unwrap_or(&tag);
        let name = names::format_title(space, title_or_tag, kind);
        let color = names::workspace_icon_color(space);
        match self.tg.create_forum_topic(forum, &name, Some(color)).await {
            Ok(thread) => {
                println!("[topics] created topic #{thread} for {pane} ({name})");
                // One lock, one save: no title-less crash window.
                self.storage
                    .insert_with_title(pane.to_string(), thread, &name);

                // Icon once by context (never flips). A missing topic
                // here means a human delete raced the create: prune so
                // the next tick recreates instead of returning dead.
                let icon = names::context_icon_emoji_id(kind);
                match self.tg.set_topic_icon(forum, thread, icon).await {
                    Ok(()) => {
                        self.storage.set_icon(pane, icon);
                    }
                    Err(e) if crate::telegram::topic_missing(&e.to_string()) => {
                        self.remove_mapping_if_thread(pane, thread);
                        return None;
                    }
                    Err(_) => {}
                }

                // F2 + D7: Identity card posted in topic header (never
                // pinned — tracked by id so status edits stay in place).
                let card =
                    crate::ui::build_identity_card_text(kind, pane, space, status, raw_title, branch);
                if let Some(mid) = self.tg.send_msg(forum, Some(thread), &card, None).await {
                    self.storage.set_pin(pane, mid);
                }

                Some(thread)
            }
            Err(e) => {
                eprintln!("[topics] failed to create topic for {pane}: {e}");
                // Roll back the tag when nothing was minted: failed
                // creates must not leak `o<n>` gaps forever.
                if self.storage.get_thread(pane).is_none() {
                    self.storage.remove_tag_if_threadless(pane);
                }
                None
            }
        }
    }

    /// Sync topic: ensure the pane's topic exists (creation + one-time
    /// context icon). Status is not reflected on the icon — it surfaces
    /// in cards and the typing indicator instead.
    pub async fn sync_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        self.sync_inner(pane, kind, space, false).await
    }

    /// Reset-owned sync: mints through the reset gate (see ensure).
    #[allow(dead_code)]
    pub async fn sync_topic_for_reset(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        self.sync_inner(pane, kind, space, true).await
    }

    async fn sync_inner(
        &self,
        pane: &str,
        kind: &str,
        space: &str,
        allow_during_reset: bool,
    ) -> Option<i64> {
        let thread = if allow_during_reset {
            self.ensure_topic_for_reset(pane, kind, space).await?
        } else {
            self.ensure_topic(pane, kind, space).await?
        };
        let forum = self.forum_id?;

        // If icon was never set for this topic (e.g. migration / boot), set it once.
        // Persisted only on success so a transient failure retries next sync.
        // A missing topic prunes now (not 60s later at the probe).
        // Unknown kind ("?") never stamps: a guessed shell icon on an
        // agent pane would permanently poison is_shell_tagged.
        if kind != "?" && self.storage.get_icon(pane).is_none() {
            let icon = names::context_icon_emoji_id(kind);
            match self.tg.set_topic_icon(forum, thread, icon).await {
                Ok(()) => {
                    self.storage.set_icon(pane, icon);
                }
                Err(e) if crate::telegram::topic_missing(&e.to_string()) => {
                    self.remove_mapping_if_thread(pane, thread);
                    return None;
                }
                Err(_) => {}
            }
        }

        Some(thread)
    }

    /// Note user-customized icon from Telegram so it is never overwritten.
    pub fn note_user_icon(&self, pane: &str, icon: &str) {
        self.storage.set_icon(pane, icon);
    }
}
