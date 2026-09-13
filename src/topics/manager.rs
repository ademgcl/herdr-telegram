use std::{collections::HashSet, sync::Mutex};
use crate::{
    telegram::client::TelegramClient,
    topics::{names, storage::TopicStorage},
};

pub struct TopicManager {
    forum_id: Option<i64>,
    storage: TopicStorage,
    tg: TelegramClient,
    /// Panes whose inherited live-status title was already stripped this
    /// process (one-time migration — steady state renames nothing, ever).
    migrated: Mutex<HashSet<String>>,
}

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            storage: TopicStorage::new(),
            tg,
            migrated: Mutex::new(HashSet::new()),
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    pub fn get_thread(&self, pane: &str) -> Option<i64> {
        self.storage.get_thread(pane)
    }

    pub fn get_pin(&self, pane: &str) -> Option<i64> {
        self.storage.get_pin(pane)
    }

    pub fn set_pin(&self, pane: &str, msg_id: i64) {
        self.storage.set_pin(pane.to_string(), msg_id);
    }

    pub fn clear_pin(&self, pane: &str) {
        self.storage.clear_pin(pane);
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.migrated.lock().unwrap().remove(pane);
        self.storage.remove(pane)
    }

    /// Ensure the pane's topic exists (`{tag} · {space}`, e.g.
    /// `o2 · herdr-telegram`) and return its thread. Titles are static
    /// identity set once at creation — never touched per message. Topics
    /// inherited with a live-status title are stripped exactly once.
    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        let forum = self.forum_id?;
        // Unknown kind (agent vanished mid-flight): never mint "?n" tags —
        // just route to the existing thread, if any.
        if kind == "?" {
            return self.storage.get_thread(pane);
        }
        let tag = self.storage.assign_tag(pane, kind);
        match self.storage.get_thread(pane) {
            Some(t) => {
                let first_sight = self.migrated.lock().unwrap().insert(pane.to_string());
                if first_sight {
                    let name = names::title(&tag, space);
                    match self.tg.rename_forum_topic(forum, t, &name).await {
                        Ok(()) => {}
                        // Already static — the common case after migration.
                        Err(e) if e.to_string().contains("TOPIC_NOT_MODIFIED") => {}
                        Err(e) => eprintln!("[topics] rename #{t} ({pane}) failed: {e}"),
                    }
                }
                Some(t)
            }
            None => {
                let name = names::title(&tag, space);
                match self.tg.create_forum_topic(forum, &name).await {
                    Ok(thread) => {
                        println!("[topics] created topic #{thread} for {pane} ({name})");
                        self.storage.insert(pane.to_string(), thread);
                        self.migrated.lock().unwrap().insert(pane.to_string());
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

    pub async fn close_topic(&self, pane: &str) {
        if let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) {
            let _ = self.tg.close_forum_topic(forum, thread).await;
        }
    }
}
