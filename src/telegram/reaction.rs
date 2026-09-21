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

/// Pure ignore verdict for reaction errors (single source): unsupported
/// chats / gone messages stay silent, everything else retries or fails.
/// Case-insensitive (errors.rs parity) — Telegram ships sentence-case
/// variants ("Reaction invalid") too. Pure for tests.
pub fn reaction_ignorable(msg: &str) -> bool {
    // Underscore/space-insensitive like retry_after (Telegram ships
    // `REACTION_INVALID` and `Reaction invalid` for the same fault).
    let low = msg.to_lowercase().replace('_', " ");
    low.contains("reactions not allowed")
        || low.contains("reactions disabled")
        || low.contains("reaction invalid")
        || low.contains("chat admin required")
        || low.contains("message to react not found")
        || low.contains("message not found")
        || low.contains("not modified")
        || low.contains("message to forward not found")
        || low.contains("message to delete not found")
        || low.contains(super::errors::NO_RIGHTS)
        || low.contains(super::errors::BOT_BLOCKED)
        || super::errors::topic_gone(msg)
}

impl TelegramClient {
    /// F7: Set an emoji reaction on a message (e.g. ❗ for blocked/stalled, ✅ for done/resumed).
    /// Pass None to clear reactions. Ignores unsupported chats / reaction errors gracefully.
    /// Retries transients + up to 3 flood-waits (send/edit parity): a tap
    /// burst must not silently drop ✅/❗ on a blip.
    pub async fn set_reaction(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: Option<&str>,
    ) -> Res<()> {
        let body = build_reaction_body(chat_id, message_id, emoji);
        let mut waits = 0;
        let mut sends = 0;
        loop {
            match self
                .call("setMessageReaction", body.clone(), Duration::from_secs(10))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if reaction_ignorable(&msg) {
                        return Ok(());
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 3 {
                            return Err(e);
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    sends += 1;
                    let retryable =
                        Self::is_transport_transient(e.as_ref()) || Self::is_transient_msg(&msg);
                    if !retryable || sends >= 3 {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
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

    #[test]
    fn test_reaction_ignorable_is_case_insensitive() {
        assert!(reaction_ignorable("Bad Request: REACTION_INVALID"));
        assert!(reaction_ignorable("Bad Request: Reaction invalid"));
        assert!(reaction_ignorable("Bad Request: reaction invalid"));
        assert!(reaction_ignorable("Forbidden: bot was kicked"));
        assert!(!reaction_ignorable("Internal Server Error"));
    }
}
