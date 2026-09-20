//! Message reactions (`setMessageReaction`) + message-effect sends.
//! Split from `client` (300-line file limit).
use super::client::TelegramClient;
use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;

pub fn build_reaction_body(chat_id: i64, message_id: i64, emoji: Option<&str>) -> Value {
    let reaction = match emoji {
        Some(e) => json!([{"type": "emoji", "emoji": e}]),
        None => json!([]),
    };
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "reaction": reaction,
    })
}

impl TelegramClient {
    /// F7: Set an emoji reaction on a message (e.g. ❗ for blocked/stalled, ✅ for done/resumed).
    /// Pass None to clear reactions. Ignores unsupported chats / reaction errors gracefully.
    /// Honors one flood-wait (then one retry): a tap burst must not silently drop ✅/❗.
    pub async fn set_reaction(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: Option<&str>,
    ) -> Res<()> {
        let body = build_reaction_body(chat_id, message_id, emoji);
        let mut waits = 0;
        loop {
            match self
                .call("setMessageReaction", body.clone(), Duration::from_secs(10))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("REACTIONS_NOT_ALLOWED")
                        || msg.contains("REACTIONS_DISABLED")
                        || msg.contains("REACTION_INVALID")
                        || msg.contains("not modified")
                        || msg.contains("message not found")
                        || msg.contains("message to forward not found")
                        || msg.contains("message to delete not found")
                        || msg.contains("message to react not found")
                        || msg.contains(super::errors::NO_RIGHTS)
                        || msg.contains(super::errors::BOT_BLOCKED)
                        || super::errors::topic_gone(&msg)
                    {
                        return Ok(());
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 1 {
                            return Err(e);
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_reaction_body() {
        let body = build_reaction_body(12345, 6789, Some("❗"));
        assert_eq!(body["chat_id"], 12345);
        assert_eq!(body["message_id"], 6789);
        assert_eq!(body["reaction"][0]["type"], "emoji");
        assert_eq!(body["reaction"][0]["emoji"], "❗");

        let body_none = build_reaction_body(12345, 6789, None);
        assert_eq!(body_none["reaction"], json!([]));
    }
}
