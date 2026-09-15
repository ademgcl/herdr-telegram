use std::{
    collections::HashMap,
    sync::Mutex,
};
use crate::{
    telegram::client::TelegramClient,
    topics::{names, storage::TopicStorage},
};

pub struct TopicManager {
    forum_id: Option<i64>,
    storage: TopicStorage,
    tg: TelegramClient,
    /// Last icon set per pane — recolors fire only on real change.
    last_icon: Mutex<HashMap<String, String>>,
}

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            storage: TopicStorage::new(),
            tg,
            last_icon: Mutex::new(HashMap::new()),
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.last_icon.lock().unwrap().remove(pane);
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
        match self.storage.get_thread(pane) {
            Some(t) => Some(t),
            None => {
                let name = names::title(&tag, space);
                match self.tg.create_forum_topic(forum, &name).await {
                    Ok(thread) => {
                        println!("[topics] created topic #{thread} for {pane} ({name})");
                        self.storage.insert(pane.to_string(), thread);
                        self.storage.set_title(pane, &name);
                        Some(thread)
                    }
                    Err(e) => {
                        eprintln!("[topics] failed to create topic for {pane}: {e}");
                        None
                    }
                }
            }
        }
    }

    /// Sync topic state: ensure the topic exists and set the state icon
    /// for this status. The NAME is never touched here — only the icon.
    /// Everything is silent (never notifies) and cache-guarded (no
    /// redundant API calls). Safe to call on every observation — icon
    /// swaps are cheap and notification-free.
    pub async fn sync_topic(&self, pane: &str, kind: &str, space: &str, status: &str) -> Option<i64> {
        let thread = self.ensure_topic(pane, kind, space).await?;
        let forum = self.forum_id?;
        let icon = names::icon_emoji_id(status).to_string();
        let due = match self.last_icon.lock().unwrap().get(pane) {
            None => true,
            Some(prev) => prev != &icon,
        };
        if due {
            match self.tg.set_topic_icon(forum, thread, &icon).await {
                Ok(()) => {
                    self.last_icon.lock().unwrap().insert(pane.to_string(), icon);
                }
                Err(e) => {
                    if crate::telegram::client::topic_missing(&e.to_string()) {
                        self.remove_mapping(pane);
                        println!("[topics] pruned missing topic #{thread} ({pane})");
                        return None;
                    }
                    eprintln!("[topics] icon topic #{thread} ({pane}) failed: {e}");
                }
            }
        }
        Some(thread)
    }

    /// Badge a live-but-agentless pane as shell: no title touch (titles
    /// sync 1:1 with herdr names), just the shell icon. Silent, idempotent.
    pub async fn mark_shell(&self, pane: &str) {
        self.sync_topic(pane, "?", "?", "shell").await;
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
    }

    /// herdr→telegram half: rename the topic when the pane's desired
    /// title drifted. Silent; stores only on success so failures retry
    /// on the next watchdog tick.
    pub async fn sync_title(&self, pane: &str, desired: &str) {
        if self.storage.get_title(pane).as_deref() == Some(desired) {
            return;
        }
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else { return };
        match self.tg.set_topic_title(forum, thread, desired).await {
            Ok(()) => {
                println!("[topics] renamed topic #{thread} ({pane}) to {desired:?}");
                self.storage.set_title(pane, desired);
            }
            Err(e) => {
                if crate::telegram::client::topic_missing(&e.to_string()) {
                    self.remove_mapping(pane);
                    println!("[topics] pruned missing topic #{thread} ({pane})");
                } else if crate::telegram::client::topic_not_modified(&e.to_string()) {
                    // Already showing it — converged, store and stay quiet
                    // instead of retry-spamming every watchdog tick.
                    self.storage.set_title(pane, desired);
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
                if crate::telegram::client::topic_missing(&e.to_string()) {
                    return true;
                }
                eprintln!("[topics] close topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }
}
