use crate::{
    telegram::client::TelegramClient,
    topics::storage::TopicStorage,
};

pub struct TopicManager {
    forum_id: Option<i64>,
    storage: TopicStorage,
    tg: TelegramClient,
}

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            storage: TopicStorage::new(),
            tg,
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.storage.remove(pane)
    }

    /// Topics are created once and NEVER renamed.
    pub fn topic_name(kind: &str, space: &str) -> String {
        format!("{kind} · {space}")
    }

    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str) -> Option<i64> {
        if let Some(t) = self.storage.get_thread(pane) {
            return Some(t);
        }
        let forum = self.forum_id?;
        let name = Self::topic_name(kind, space);
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

    pub async fn close_topic(&self, pane: &str) {
        if let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) {
            let _ = self.tg.close_forum_topic(forum, thread).await;
        }
    }
}
