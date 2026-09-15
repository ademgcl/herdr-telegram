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
    pub(crate) storage: TopicStorage,
    pub(crate) tg: TelegramClient,
    pub(crate) last_title_write: Mutex<HashMap<String, Instant>>,
    pub(crate) creating: Mutex<HashSet<String>>,
}

mod lifecycle;
mod titles;

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            storage: TopicStorage::new(),
            tg,
            last_title_write: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashSet::new()),
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    /// Ensure the pane's topic exists and return its thread. New topics
    /// open under the herdr pane id; the watchdog's 1:1 title sync
    /// renames to the pane label when one is set. Manual renames are
    /// never overwritten blindly — they flow back as pane labels
    /// (`forum_topic_edited` → `pane.rename`), and the stored title
    /// absorbs our own sync echoes.
    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        let forum = self.forum_id?;
        // Unknown kind (agent vanished mid-flight): never mint topics —
        // just route to the existing thread, if any.
        if kind == "?" {
            return self.storage.get_thread(pane);
        }
        // New topics open under the friendly default (stable tag + space);
        // the watchdog writes it into the herdr pane label when unlabeled,
        // so the default name is herdr-tracked from the start.
        let tag = self.storage.assign_tag(pane, kind);
        if let Some(t) = self.storage.get_thread(pane) {
            return Some(t);
        }
        // Single-flight: a concurrent ensure for this same new pane may
        // already be creating. The loser waits for the winner's insert
        // (bounded — a slow network just defers to the next tick).
        if !self
            .creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pane.to_string())
        {
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Some(t) = self.storage.get_thread(pane) {
                    return Some(t);
                }
            }
            return self.storage.get_thread(pane);
        }
        let name = names::title(&tag, space, kind);
        let out = match self.tg.create_forum_topic(forum, &name).await {
            Ok(thread) => {
                println!("[topics] created topic #{thread} for {pane} ({name})");
                self.storage.insert(pane.to_string(), thread);
                self.storage.set_title(pane, &name);

                // Set initial icon once according to context/kind (never flips on status).
                // Persisted only on success so a transient failure retries next sync.
                let icon = names::context_icon_emoji_id(kind);
                if self.tg.set_topic_icon(forum, thread, icon).await.is_ok() {
                    self.storage.set_icon(pane, icon);
                }

                // F2: Identity card posted and pinned in topic header
                let card =
                    format!("📌 **{kind}** · `{pane}`\nWorkspace: `{space}`\nStatus: 💬 ready");
                if let Some(mid) = self.tg.send_msg(forum, Some(thread), &card, None).await {
                    let _ = self.tg.pin_msg(forum, mid).await;
                }

                Some(thread)
            }
            Err(e) => {
                eprintln!("[topics] failed to create topic for {pane}: {e}");
                None
            }
        };
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        out
    }

    /// Sync topic: ensure the pane's topic exists (creation + one-time
    /// context icon). Status is not reflected on the icon — it surfaces
    /// in cards, pins and the typing indicator instead.
    pub async fn sync_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        let thread = self.ensure_topic(pane, kind, space).await?;
        let forum = self.forum_id?;

        // If icon was never set for this topic (e.g. migration / boot), set it once.
        // Persisted only on success so a transient failure retries next sync.
        if self.storage.get_icon(pane).is_none() {
            let icon = names::context_icon_emoji_id(kind);
            if self.tg.set_topic_icon(forum, thread, icon).await.is_ok() {
                self.storage.set_icon(pane, icon);
            }
        }

        Some(thread)
    }

    /// Note user-customized icon from Telegram so it is never overwritten.
    pub fn note_user_icon(&self, pane: &str, icon: &str) {
        self.storage.set_icon(pane, icon);
    }
}
