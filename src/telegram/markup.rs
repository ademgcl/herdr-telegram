//! Markup-only card edits (editMessageReplyMarkup): claim/strip buttons
//! without touching text. Split from `messages` (300-line file limit).
//!
//! Pending-tap claims strip the tapped card's buttons BEFORE the slow
//! herdr roundtrips, so a tap feels instant; every reconcile path then
//! overwrites the card (or it is already buttonless — converged by
//! construction, never ghost buttons). Fail-closed like `try_edit_msg`.
use super::client::TelegramClient;
use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;

pub fn build_markup_params(chat_id: i64, message_id: i64, keyboard: Option<Value>) -> Value {
    let mut params = json!({"chat_id": chat_id, "message_id": message_id});
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    params
}

impl TelegramClient {
    /// Fallible markup edit: caller falls back (fresh send / strip) when
    /// the message is gone. Same fatal mapping as `try_edit_msg`.
    pub async fn try_edit_markup(
        &self,
        chat_id: i64,
        message_id: i64,
        keyboard: Option<Value>,
    ) -> Res<()> {
        let params = build_markup_params(chat_id, message_id, keyboard);
        let mut sends = 0;
        let mut waits = 0;
        loop {
            match self
                .call("editMessageReplyMarkup", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("message is not modified") {
                        return Ok(());
                    }
                    if super::errors::topic_missing(&msg)
                        || msg.contains("message to edit not found")
                        || msg.contains("message can't be edited")
                        || msg.contains("not enough rights")
                        || msg.contains("bot was blocked")
                    {
                        eprintln!("editMessageReplyMarkup fatal: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 3 {
                            return Err(msg.into());
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    sends += 1;
                    if sends >= 3 {
                        eprintln!("editMessageReplyMarkup failed: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Best-effort button strip: never fails the caller. Pending claims
    /// and fail-closed converges share it.
    pub async fn strip_buttons(&self, chat_id: i64, message_id: i64) {
        let _ = self
            .try_edit_markup(chat_id, message_id, Some(Value::Array(Vec::new())))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_markup_params_strips_with_empty_kb() {
        let p = build_markup_params(1, 2, Some(Value::Array(Vec::new())));
        assert_eq!(p["chat_id"], 1);
        assert_eq!(p["message_id"], 2);
        assert_eq!(p["reply_markup"]["inline_keyboard"], Value::Array(Vec::new()));
        let q = build_markup_params(1, 2, None);
        assert!(q.get("reply_markup").is_none());
    }
}
