use super::client::TelegramClient;
use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;

pub async fn get_updates(tg: &TelegramClient, offset: u64, poll_secs: i64) -> Res<Vec<Value>> {
    let r = tg
        .call(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": poll_secs,
                "limit": 100,
                "allowed_updates": ["message", "edited_message", "callback_query", "my_chat_member"],
            }),
            Duration::from_secs(poll_secs as u64 + 10),
        )
        .await?;
    Ok(r.as_array().cloned().unwrap_or_default())
}
