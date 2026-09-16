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

        // Flood-waits have their own budget: honoring a wait must not
        // consume one of the 3 sends (that would drop long-waited
        // messages, possibly final cards).
        let mut sends = 0;
        let mut waits = 0;
        loop {
            match self
                .call("sendMessage", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(v) => return v["message_id"].as_i64(),
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("sendMessage failed: {}", self.redact(&msg));
                    // Flood-waits are honored as-is under their own budget:
                    // capping or mistreating them drops messages (possibly
                    // final cards) or risks a ban. Long waits stall this
                    // task, but the watcher's epoch checks abort after.
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 3 {
                            break;
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    sends += 1;
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| {
                            re.is_connect()
                                || re.is_timeout()
                                || re.status().map(|s| s.is_server_error()).unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if !retryable || sends >= 3 {
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
        let mut sends = 0;
        let mut waits = 0;
        loop {
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
                    // just amplifies outage load. Dead topics fail fast via
                    // the shared predicate instead of 3x per tick.
                    if super::errors::topic_missing(&msg)
                        || msg.contains("message to edit not found")
                        || msg.contains("message can't be edited")
                        || msg.contains("not enough rights")
                        || msg.contains("bot was blocked")
                    {
                        eprintln!("editMessageText fatal: {}", self.redact(&msg));
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
                        eprintln!("editMessageText failed: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
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

    /// F6: Copy a message to another topic thread without forward headers.
    pub async fn copy_msg(
        &self,
        chat_id: i64,
        from_chat_id: i64,
        message_id: i64,
        thread_id: Option<i64>,
    ) -> Option<i64> {
        let mut params = json!({
            "chat_id": chat_id,
            "from_chat_id": from_chat_id,
            "message_id": message_id,
        });
        if let Some(th) = thread_id {
            params["message_thread_id"] = json!(th);
        }
        match self
            .call_retrying("copyMessage", params, Duration::from_secs(15))
            .await
        {
            Ok(v) => v["message_id"].as_i64(),
            Err(e) => {
                eprintln!("copyMessage {message_id} failed: {}", self.redact(&e.to_string()));
                None
            }
        }
    }

    /// Unpin a message (one-time cleanup helper).
    #[allow(dead_code)]
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
