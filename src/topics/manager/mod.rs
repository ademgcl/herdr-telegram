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
};

pub struct TopicManager {
    pub(crate) forum_id: Option<i64>,
    pub(crate) socket: Option<String>,
    pub(crate) storage: TopicStorage,
    pub(crate) tg: TelegramClient,
    pub(crate) last_kind: Mutex<HashMap<String, String>>,
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
            last_kind: Mutex::new(HashMap::new()),
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
    /// fresh remint. Aux kind memory dies with the mapping (a remint
    /// must re-observe, never inherit a stale flip; dead panes must not
    /// accumulate here unbounded). `creating` is deliberately UNTOUCHED:
    /// a racing ensure may hold it mid-RPC (its CreatingGuard RAII owns
    /// the lifecycle) — clearing here would let a second ensure claim
    /// and double-mint an orphan topic.
    pub fn remove_mapping_if_thread(&self, pane: &str, thread: i64) -> bool {
        if !self.storage.remove_if_thread(pane, thread) {
            return false;
        }
        // Kind memory dies with the mapping: a remint must re-observe,
        // never inherit a stale flip (and dead panes must not
        // accumulate here unbounded).
        self.last_kind
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        true
    }

    pub fn get_pin(&self, pane: &str) -> Option<i64> {
        self.storage.get_pin(pane)
    }

    /// Single-lock compare-and-set pin (storage guard): stores only when
    /// the thread still matches — a stale send must never clobber a
    /// fresh remint's pin with a dead mid.
    pub fn set_pin_if_thread(&self, pane: &str, thread: i64, mid: i64) -> bool {
        self.storage.set_pin_if_thread(pane, thread, mid)
    }

    /// Ensure the pane's topic exists and return its thread. New topics
    /// open under the herdr pane id; the watchdog's 1:1 title sync
    /// renames to the pane label when one is set. Manual renames are
    /// never overwritten blindly — they flow back as pane labels
    /// (`forum_topic_edited` → `pane.rename`), and the stored title
    /// absorbs our own sync echoes.
    ///
    /// Plus prune signal: true when a just-minted mapping was pruned
    /// this call (a human delete raced the create — caller retires its
    /// dialog generation).
    async fn ensure_inner(&self, pane: &str, kind: &str, space: &str) -> (Option<i64>, bool) {
        let Some(forum) = self.forum_id else {
            return (None, false);
        };
        // A paced reset is rebuilding the map: reuse only, never mint —
        // anything created now is wiped mid-reset into an orphan (later
        // re-minted as a double). Deferred panes mint on post-reset ticks.
        if crate::handlers::reset::is_resetting() {
            return (self.storage.get_thread(pane), false);
        }
        // Unknown kind (agent vanished mid-flight): never mint topics —
        // just route to the existing thread, if any.
        if kind == "?" {
            return (self.storage.get_thread(pane), false);
        }
        if let Some(t) = self.storage.get_thread(pane) {
            return (Some(t), false);
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
                    return (Some(t), false);
                }
            }
            return (self.storage.get_thread(pane), false);
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
        if crate::handlers::reset::is_resetting() {
            return (self.storage.get_thread(pane), false);
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

        // Mint with the stable tag — never the agent terminal title
        // (titles are 1:1 with herdr TAB names, watchdog converges ≤60s;
        // the terminal title lives only in the identity card below).
        let name = names::format_title(space, &tag, kind);
        let color = names::workspace_icon_color(space);
        match self.tg.create_forum_topic(forum, &name, Some(color)).await {
            Ok(thread) => {
                println!("[topics] created topic #{thread} for {pane} ({name})");
                // One lock, one save: no title-less crash window.
                self.storage
                    .insert_with_title(pane.to_string(), thread, &name);

                // Icon by live kind at mint (flips converge next
                // watchdog tick). A missing topic here means a human
                // delete raced the create: prune so the next tick
                // recreates instead of returning dead.
                let icon = names::context_icon_emoji_id(kind);
                match self.tg.set_topic_icon(forum, thread, icon).await {
                    Ok(()) => {
                        self.storage.set_icon(pane, icon);
                    }
                    Err(e) if crate::telegram::topic_missing(&e.to_string()) => {
                        self.remove_mapping_if_thread(pane, thread);
                        return (None, true);
                    }
                    Err(_) => {}
                }

                // F2 + D7: Identity card posted in topic header (never
                // pinned — tracked by id so status edits stay in place).
                let card = crate::ui::build_identity_card_text(
                    kind, pane, space, status, raw_title, branch,
                );
                if let Some(mid) = self.tg.send_msg(forum, Some(thread), &card, None).await
                    && !self.storage.set_pin_if_thread(pane, thread, mid)
                {
                    println!("[topics] pin reminted during mint for {pane} — dropping stale mid");
                }

                (Some(thread), false)
            }
            Err(e) => {
                eprintln!(
                    "[topics] failed to create topic for {pane}: {}",
                    self.tg.redact(&e.to_string())
                );
                // Roll back the tag when nothing was minted: failed
                // creates must not leak `o<n>` gaps forever.
                if self.storage.get_thread(pane).is_none() {
                    self.storage.remove_tag_if_threadless(pane);
                }
                (None, false)
            }
        }
    }

    /// Sync topic: ensure the pane's topic exists (creation stamps the
    /// kind icon; flips converge on the watchdog tick). Status is not
    /// reflected on the icon — it surfaces in cards and the typing
    /// indicator instead. Returns the thread plus prune signal: true
    /// when the mapping was pruned this call (caller retires its dialog
    /// generation — every call site does).
    pub async fn sync_topic_prune(
        &self,
        pane: &str,
        kind: &str,
        space: &str,
    ) -> (Option<i64>, bool) {
        self.sync_inner(pane, kind, space).await
    }

    async fn sync_inner(&self, pane: &str, kind: &str, space: &str) -> (Option<i64>, bool) {
        let (thread_opt, pruned) = self.ensure_inner(pane, kind, space).await;
        let Some(thread) = thread_opt else {
            return (None, pruned);
        };
        let Some(forum) = self.forum_id else {
            return (None, pruned);
        };

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
                    return (None, true);
                }
                Err(_) => {}
            }
        }

        (Some(thread), pruned)
    }

    /// Note user-customized icon from Telegram so it is never overwritten.
    pub fn note_user_icon(&self, pane: &str, icon: &str) {
        self.storage.set_icon(pane, icon);
    }
}
