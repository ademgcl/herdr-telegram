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

    /// Ensure the pane's topic exists and return its thread. The auto
    /// title (`{tag} · {space}`, pure text) is set exactly once — at
    /// creation. After that the name belongs to the owner: manual
    /// renames are NEVER overwritten (icons still track status below).
    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        let forum = self.forum_id?;
        // Unknown kind (agent vanished mid-flight): never mint "?n" tags —
        // just route to the existing thread, if any.
        if kind == "?" {
            return self.storage.get_thread(pane);
        }
        let tag = self.storage.assign_tag(pane, kind);
        match self.storage.get_thread(pane) {
            Some(t) => Some(t),
            None => {
                let name = names::title(&tag, space);
                match self.tg.create_forum_topic(forum, &name).await {
                    Ok(thread) => {
                        println!("[topics] created topic #{thread} for {pane} ({name})");
                        self.storage.insert(pane.to_string(), thread);
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
            self.tg.set_topic_icon(forum, thread, &icon).await;
            self.last_icon.lock().unwrap().insert(pane.to_string(), icon);
        }
        Some(thread)
    }

    /// Badge a live-but-agentless pane as shell: no title touch (tags
    /// stay stable for re-entry), just the shell icon. Silent, idempotent.
    pub async fn mark_shell(&self, pane: &str) {
        self.sync_topic(pane, "?", "?", "shell").await;
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

    pub async fn close_topic(&self, pane: &str) {
        if let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) {
            let _ = self.tg.close_forum_topic(forum, thread).await;
        }
    }
}
