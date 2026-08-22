use std::time::Duration;
use serde_json::{json, Value};
use crate::{state::AppState, telegram::client::TelegramClient, types::Res};

pub async fn get_updates(tg: &TelegramClient, offset: u64, poll_secs: i64) -> Res<Vec<Value>> {
    let r = tg
        .call(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": poll_secs,
                "limit": 100,
                "allowed_updates": ["message", "callback_query", "my_chat_member"],
            }),
            Duration::from_secs(poll_secs as u64 + 10),
        )
        .await?;
    Ok(r.as_array().cloned().unwrap_or_default())
}

pub async fn discard_backlog(s: &AppState) -> Res<()> {
    let backlog = get_updates(&s.tg, 0, 0).await?;
    let max_id = backlog.last().and_then(|u| u["update_id"].as_u64());
    if let Some(id) = max_id {
        *s.offset.lock().await = id + 1;
    }
    if !backlog.is_empty() {
        println!("[tg] discarded {} stale update(s)", backlog.len());
    }
    Ok(())
}
