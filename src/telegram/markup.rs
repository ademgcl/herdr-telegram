//! Markup-only card edits (editMessageReplyMarkup): claim/strip buttons
//! without touching text. Split from `messages` (300-line file limit).
//!
//! Strips are single-attempt best-effort (no retry): they run inside the
//! tap's single-flight hold, so a degraded-Telegram retry storm must
//! never wedge the pane behind a doomed UX call. Convergence never
//! depends on a strip landing — every outcome arm overwrites the card
//! (or the delayed heal re-renders it).
use super::client::TelegramClient;
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
    /// Best-effort button strip: one attempt, never fails the caller.
    /// Pending claims and fail-closed converges share it.
    pub async fn strip_buttons(&self, chat_id: i64, message_id: i64) {
        let params = build_markup_params(chat_id, message_id, Some(Value::Array(Vec::new())));
        let _ = self
            .call("editMessageReplyMarkup", params, Duration::from_secs(10))
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
        assert_eq!(
            p["reply_markup"]["inline_keyboard"],
            Value::Array(Vec::new())
        );
        let q = build_markup_params(1, 2, None);
        assert!(q.get("reply_markup").is_none());
    }
}
