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
    forum_id: Option<i64>,
    storage: TopicStorage,
    tg: TelegramClient,
    last_title_write: Mutex<HashMap<String, Instant>>,
    creating: Mutex<HashSet<String>>,
}

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

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.last_title_write.lock().unwrap().remove(pane);
        self.creating.lock().unwrap().remove(pane);
        self.storage.remove(pane)
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
        if !self.creating.lock().unwrap().insert(pane.to_string()) {
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Some(t) = self.storage.get_thread(pane) {
                    return Some(t);
                }
            }
            return self.storage.get_thread(pane);
        }
        let name = names::title(&tag, space);
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
        self.creating.lock().unwrap().remove(pane);
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

    /// F1: Reopen a closed forum topic (e.g. when agent transitions to working).
    pub async fn reopen_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.reopen_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    return true;
                }
                eprintln!("[topics] reopen topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    /// Badge a live-but-agentless pane as shell: no title touch (titles
    /// sync 1:1 with herdr names), just ensures the topic.
    pub async fn mark_shell(&self, pane: &str) {
        self.sync_topic(pane, "shell", "?").await;
    }

    /// Stable short tag for this pane (`o2`) — backs the friendly
    /// default title for unlabeled panes.
    pub fn tag_for(&self, pane: &str, kind: &str) -> String {
        self.storage.assign_tag(pane, kind)
    }

    /// Last synced 1:1 title for this pane (herdr label or pane id).
    pub fn topic_title(&self, pane: &str) -> Option<String> {
        self.storage.get_title(pane)
    }

    /// Record a title the telegram side already shows (native rename):
    /// no API call, just the echo loop-guard.
    pub fn note_title(&self, pane: &str, title: &str) {
        self.storage.set_title(pane, title);
        self.last_title_write
            .lock()
            .unwrap()
            .insert(pane.to_string(), Instant::now());
    }

    /// herdr→telegram half: rename the topic when the pane's desired
    /// title drifted. Silent; stores only on success so failures retry
    /// on the next watchdog tick.
    pub async fn sync_title(&self, pane: &str, desired: &str) {
        if self.storage.get_title(pane).as_deref() == Some(desired) {
            return;
        }
        if let Some(t) = self.last_title_write.lock().unwrap().get(pane)
            && t.elapsed() < std::time::Duration::from_secs(5)
        {
            println!("[topics] skip rename {pane}: recent title write");
            return;
        }
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return;
        };
        match self.tg.set_topic_title(forum, thread, desired).await {
            Ok(()) => {
                println!("[topics] renamed topic #{thread} ({pane}) to {desired:?}");
                self.storage.set_title(pane, desired);
                self.last_title_write
                    .lock()
                    .unwrap()
                    .insert(pane.to_string(), Instant::now());
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping(pane);
                    println!("[topics] pruned missing topic #{thread} ({pane})");
                } else if crate::telegram::topic_not_modified(&e.to_string()) {
                    // Already showing it — converged, store and stay quiet
                    // instead of retry-spamming every watchdog tick.
                    self.storage.set_title(pane, desired);
                    self.last_title_write
                        .lock()
                        .unwrap()
                        .insert(pane.to_string(), Instant::now());
                } else {
                    eprintln!("[topics] rename topic #{thread} ({pane}) failed: {e}");
                }
            }
        }
    }

    /// One-time cleanup of the retired pinned-status era: unpin leftovers.
    /// No-op once storage is clean.
    pub async fn retire_pins(&self) {
        let Some(forum) = self.forum_id else { return };
        for (pane, mid) in self.storage.take_pins() {
            println!("[topics] unpinning retired status pin #{mid} ({pane})");
            self.tg.unpin_msg(forum, mid).await;
        }
    }

    pub async fn close_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.close_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    return true;
                }
                eprintln!("[topics] close topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    pub async fn delete_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.delete_forum_topic(forum, thread).await {
            Ok(()) => {
                self.remove_mapping(pane);
                println!("[topics] deleted topic #{thread} ({pane})");
                true
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping(pane);
                    return true;
                }
                eprintln!("[topics] delete topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    /// Restore a mapping wiped by `clear_all` (reset retry path):
    /// the Telegram topic survived, so re-sync reuses it instead of
    /// minting a duplicate. Titles/tags reconverge on the next tick.
    pub fn restore_mapping(&self, pane: String, thread: i64) {
        self.storage.insert(pane, thread);
    }

    pub fn clear_all(&self) {
        self.last_title_write.lock().unwrap().clear();
        self.creating.lock().unwrap().clear();
        self.storage.clear_all();
    }
}
