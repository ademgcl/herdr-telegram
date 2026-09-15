use super::client::TelegramClient;
use crate::{types::Res, ui::fit_msg};
use serde_json::{Value, json};
use std::time::Duration;

impl TelegramClient {
    pub async fn send_msg(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
        keyboard: Option<Value>,
    ) -> Option<i64> {
        let body = fit_msg(text);
        let mut params = json!({"chat_id": chat_id, "text": body});
        if let Some(th) = thread_id {
            params["message_thread_id"] = json!(th);
        }
        if let Some(kb) = keyboard {
            params["reply_markup"] = json!({"inline_keyboard": kb});
        }

        for attempt in 0..3 {
            match self
                .call("sendMessage", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(v) => return v["message_id"].as_i64(),
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!(
                        "sendMessage failed (attempt {}): {}",
                        attempt + 1,
                        self.redact(&msg)
                    );
                    // Flood-waits are honored up to 5 min: dropping a
                    // long wait silently loses the message (possibly a
                    // final card). Past that, one last attempt still runs
                    // below instead of giving up outright.
                    if let Some(wait) = Self::retry_after(&msg).filter(|w| w.as_secs() <= 300) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| re.is_connect() || re.is_timeout())
                        .unwrap_or(false);
                    if !retryable || attempt == 2 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        None
    }

    pub async fn edit_msg(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<Value>,
    ) {
        let _ = self.try_edit_msg(chat_id, message_id, text, keyboard).await;
    }

    /// Fallible edit for live/final cards: caller falls back to a fresh
    /// send when the message is gone (deleted topic, etc.).
    pub async fn try_edit_msg(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<Value>,
    ) -> Res<()> {
        let body = fit_msg(text);
        let mut params = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": body,
        });
        if let Some(kb) = keyboard {
            params["reply_markup"] = json!({"inline_keyboard": kb});
        }
        for attempt in 0..3 {
            match self
                .call("editMessageText", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("message is not modified") {
                        return Ok(());
                    }
                    // Known-fatal: retrying a deleted/uneditable message
                    // just amplifies outage load.
                    if msg.contains("message to edit not found")
                        || msg.contains("message can't be edited")
                        || msg.contains("not enough rights")
                        || msg.contains("bot was blocked")
                    {
                        eprintln!("editMessageText fatal: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
                    if let Some(wait) = Self::retry_after(&msg).filter(|w| w.as_secs() <= 300) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if attempt == 2 {
                        eprintln!("editMessageText failed: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
        Err("editMessageText retries exhausted".into())
    }

    /// "typing…" indicator — lasts ~5s, repeat to sustain. Zero clutter.
    pub async fn typing(&self, chat_id: i64, thread_id: Option<i64>) {
        let mut params = json!({"chat_id": chat_id, "action": "typing"});
        if let Some(th) = thread_id {
            params["message_thread_id"] = json!(th);
        }
        let _ = self
            .call("sendChatAction", params, Duration::from_secs(10))
            .await;
    }

    pub async fn answer_callback(&self, cbq_id: &str) {
        let _ = self
            .call(
                "answerCallbackQuery",
                json!({"callback_query_id": cbq_id}),
                Duration::from_secs(10),
            )
            .await;
    }

    /// Unpin a message (one-time cleanup helper).
    pub async fn unpin_msg(&self, chat_id: i64, message_id: i64) {
        if let Err(e) = self
            .call(
                "unpinChatMessage",
                json!({"chat_id": chat_id, "message_id": message_id}),
                Duration::from_secs(15),
            )
            .await
        {
            eprintln!("unpinChatMessage failed: {}", self.redact(&e.to_string()));
        }
    }
}
