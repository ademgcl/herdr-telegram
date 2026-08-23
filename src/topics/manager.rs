use std::collections::HashMap;
use serde_json::Value;
use crate::{
    herdr::client::{get_agent, list_workspaces},
    telegram::client::TelegramClient,
    topics::storage::TopicStorage,
};

pub struct TopicManager {
    forum_id: Option<i64>,
    socket_path: String,
    storage: TopicStorage,
    tg: TelegramClient,
}

impl TopicManager {
    pub fn new(forum_id: Option<i64>, socket_path: String, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            socket_path,
            storage: TopicStorage::new(),
            tg,
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    #[allow(dead_code)]
    pub fn thread_of_pane(&self, pane: &str) -> Option<i64> {
        self.storage.get_thread(pane)
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.storage.remove(pane)
    }

    /// True if this flipped the pane to unread (new unseen activity in its topic).
    pub fn mark_unread(&self, pane: &str) -> bool {
        self.storage.mark_unread(pane)
    }

    /// True if this flipped the pane to read (owner interacted with its topic).
    pub fn mark_read(&self, pane: &str) -> bool {
        self.storage.mark_read(pane)
    }

    fn is_unread(&self, pane: &str) -> bool {
        self.storage.is_unread(pane)
    }

    /// Titles stay static (kind · space) to avoid churn; only the 📩 unread
    /// marker ever toggles them.
    pub fn topic_name(kind: &str, space: &str, _status: &str) -> String {
        format!("{kind} · {space}")
    }

    fn titled_name(&self, pane: &str, kind: &str, space: &str, status: &str) -> String {
        let base = Self::topic_name(kind, space, status);
        if self.is_unread(pane) {
            format!("📩 {base}")
        } else {
            base
        }
    }

    pub async fn ensure_topic(&self, pane: &str, kind: &str, space: &str, status: &str) -> Option<i64> {
        if let Some(t) = self.storage.get_thread(pane) {
            return Some(t);
        }
        let forum = self.forum_id?;
        let name = self.titled_name(pane, kind, space, status);
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

    pub async fn update_topic_title(&self, pane: &str, kind: &str, space: &str, status: &str) {
        let Some(forum) = self.forum_id else { return };
        let Some(thread) = self.ensure_topic(pane, kind, space, status).await else { return };
        let name = self.titled_name(pane, kind, space, status);
        let _ = self.tg.edit_forum_topic(forum, thread, &name).await;
    }

    pub async fn close_topic(&self, pane: &str, kind: &str, space: &str) {
        let Some(forum) = self.forum_id else { return };
        let Some(thread) = self.storage.get_thread(pane) else { return };
        let closed_name = format!("⚪ {kind} · {space} [exited]");
        let _ = self.tg.edit_forum_topic(forum, thread, &closed_name).await;
        let _ = self.tg.close_forum_topic(forum, thread).await;
    }

    #[allow(dead_code)]
    pub async fn send_agent_msg(
        &self,
        pane: &str,
        text: &str,
        keyboard: Option<Value>,
        fallback_owners: &[i64],
    ) -> Option<i64> {
        if let Some(forum) = self.forum_id {
            let info = get_agent(&self.socket_path, pane).await.ok();
            let spaces = list_workspaces(&self.socket_path).await.unwrap_or_default();
            let (kind, ws_id, status) = match &info {
                Some(a) => (a.kind.clone(), a.ws.clone(), a.status.clone()),
                None => ("?".into(), "?".into(), "unknown".into()),
            };
            let space_label = spaces
                .iter()
                .find(|s| s.id == ws_id)
                .map(|s| s.label.clone())
                .unwrap_or(ws_id);

            if let Some(thread) = self.ensure_topic(pane, &kind, &space_label, &status).await {
                if let Some(mid) = self.tg.send_msg(forum, Some(thread), text, keyboard.clone()).await {
                    return Some(mid);
                }
                // topic may have been deleted manually, retry once
                self.storage.remove(pane);
                if let Some(thread) = self.ensure_topic(pane, &kind, &space_label, &status).await {
                    return self.tg.send_msg(forum, Some(thread), text, keyboard).await;
                }
                return None;
            }
        }

        let mut last_mid = None;
        for id in fallback_owners {
            let mid = self.tg.send_msg(*id, None, text, keyboard.clone()).await;
            if mid.is_some() {
                last_mid = mid;
            }
        }
        last_mid
    }
}
