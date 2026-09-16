//! Forum pin primitives: pin-a-message + unpin-all-in-topic. Split
//! from `client` (300-line file limit). Errors that mean "nothing to
//! do" (topic gone, not modified, no rights) return Ok.
use super::client::TelegramClient;
use crate::types::Res;
use serde_json::json;
use std::time::Duration;

impl TelegramClient {
    /// F2: Pin a message in a chat/topic without notification.
    pub async fn pin_msg(&self, chat_id: i64, message_id: i64) -> Res<()> {
        self.call_retrying(
            "pinChatMessage",
            json!({
                "chat_id": chat_id,
                "message_id": message_id,
                "disable_notification": true
            }),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
    }

    /// F3: Unpin all messages in a forum topic in a single call.
    pub async fn unpin_all_forum_topic_messages(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "unpinAllForumTopicMessages",
                json!({
                    "chat_id": chat_id,
                    "message_thread_id": thread_id,
                }),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if super::errors::topic_missing(&msg)
                    || msg.contains("not modified")
                    || msg.contains("not enough rights")
                {
                    return Ok(());
                }
                Err(e)
            }
        }
    }
}
