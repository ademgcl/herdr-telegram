use super::client::TelegramClient;
use crate::types::Res;
use serde_json::json;
use std::time::Duration;

impl TelegramClient {
    pub async fn create_forum_topic(&self, chat_id: i64, name: &str) -> Res<i64> {
        let res = self
            .call_retrying(
                "createForumTopic",
                json!({"chat_id": chat_id, "name": name}),
                Duration::from_secs(15),
            )
            .await?;
        res["message_thread_id"]
            .as_i64()
            .ok_or_else(|| "missing message_thread_id in createForumTopic response".into())
    }

    pub async fn delete_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "deleteForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if super::errors::topic_missing(&msg) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    pub async fn close_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "closeForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if super::errors::topic_missing(&msg) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    pub async fn reopen_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "reopenForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if super::errors::topic_missing(&msg)
                    || msg.contains("TOPIC_NOT_MODIFIED")
                    || msg.contains("not closed")
                {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    /// Rename a forum topic — the herdr→telegram half of 1:1 title
    /// sync. Silent (never notifies); our own edit echoes back as
    /// `forum_topic_edited`, which the stored-title compare absorbs.
    pub async fn set_topic_title(&self, chat_id: i64, thread_id: i64, name: &str) -> Res<()> {
        self.call_retrying(
            "editForumTopic",
            json!({"chat_id": chat_id, "message_thread_id": thread_id, "name": name}),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
    }

    /// Set a forum topic's custom-emoji icon — the silent state signal.
    /// Unlike `icon_color` (create-only, ignored on edit), this applies
    /// AND renders on edit (verified live). Never notifies.
    pub async fn set_topic_icon(&self, chat_id: i64, thread_id: i64, emoji_id: &str) -> Res<()> {
        if let Err(e) = self
            .call_retrying(
                "editForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id, "icon_custom_emoji_id": emoji_id}),
                Duration::from_secs(15),
            )
            .await
        {
            let msg = e.to_string();
            if msg.contains("message is not modified") || msg.contains("NOT_MODIFIED") {
                return Ok(());
            }
            eprintln!("set_topic_icon #{thread_id} failed: {}", self.redact(&msg));
            return Err(msg.into());
        }
        Ok(())
    }
}
