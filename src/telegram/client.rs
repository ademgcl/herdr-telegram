use crate::{types::Res, ui::fit_msg};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct TelegramClient {
    token: String,
    http: reqwest::Client,
}

impl TelegramClient {
    pub fn new(token: String) -> Res<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { token, http })
    }

    pub fn redact(&self, s: &str) -> String {
        s.replace(&self.token, "<redacted>")
    }

    pub async fn call(&self, method: &str, body: Value, timeout: Duration) -> Res<Value> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .timeout(timeout)
            .send()
            .await?;
        let v: Value = resp.json().await?;
        if v["ok"].as_bool() != Some(true) {
            let desc = v["description"].as_str().unwrap_or("telegram error");
            return Err(desc.to_string().into());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Telegram flood-wait: "Too Many Requests: retry after N" — honor it
    /// instead of silently dropping the message.
    fn retry_after(e: &str) -> Option<Duration> {
        let tail = e.rsplit("retry after").next()?.trim();
        tail.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<u64>()
            .ok()
            .map(|s| Duration::from_secs(s + 1))
    }

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
                    if let Some(wait) = Self::retry_after(&msg).filter(|w| w.as_secs() <= 60) {
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
                    if let Some(wait) = Self::retry_after(&msg).filter(|w| w.as_secs() <= 60) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if attempt == 2 {
                        eprintln!("editMessageText failed: {}", self.redact(&msg));
                        return Err(msg.into());
                    }
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

    pub async fn create_forum_topic(&self, chat_id: i64, name: &str) -> Res<i64> {
        let res = self
            .call(
                "createForumTopic",
                json!({"chat_id": chat_id, "name": name}),
                Duration::from_secs(15),
            )
            .await?;
        res["message_thread_id"]
            .as_i64()
            .ok_or_else(|| "missing message_thread_id in createForumTopic response".into())
    }

    pub async fn close_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        self.call(
            "closeForumTopic",
            json!({"chat_id": chat_id, "message_thread_id": thread_id}),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
    }

    /// Rename a forum topic — the herdr→telegram half of 1:1 title
    /// sync. Silent (never notifies); our own edit echoes back as
    /// `forum_topic_edited`, which the stored-title compare absorbs.
    pub async fn set_topic_title(&self, chat_id: i64, thread_id: i64, name: &str) -> Res<()> {
        self.call(
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
            .call(
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

    pub async fn set_my_commands(&self) -> Res<()> {
        let _ = self
            .call(
                "setMyCommands",
                json!({"commands": [
                    {"command": "agents", "description": "open control panel (spaces + agents)"},
                    {"command": "spawn", "description": "spawn a new agent: /spawn <kind> [space]"},
                    {"command": "space", "description": "new space + shell topic"},
                    {"command": "model", "description": "current model + free-Zen picker"},
                    {"command": "quit", "description": "drop the agent to a shell"},
                    {"command": "kill", "description": "close the pane completely"},
                    {"command": "shell", "description": "open a fresh shell pane"},
                    {"command": "read", "description": "recent output of focused agent"},
                    {"command": "cancel", "description": "abort pending prompts / keys-mode"},
                    {"command": "keys", "description": "/keys <pane> y enter — send raw keys"},
                    {"command": "help", "description": "how to drive agents from here"},
                ]}),
                Duration::from_secs(15),
            )
            .await;
        Ok(())
    }
}

pub fn topic_missing(err: &str) -> bool {
    err.contains("TOPIC_ID_INVALID")
}

/// Already showing that title — converged, not a failure. Callers store
/// and stay quiet instead of retry-spamming every tick.
pub fn topic_not_modified(err: &str) -> bool {
    err.contains("TOPIC_NOT_MODIFIED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> TelegramClient {
        TelegramClient::new("fake-token-123".to_string()).expect("client build")
    }

    #[test]
    fn redact_replaces_token() {
        let c = test_client();
        let out = c.redact("https://api.telegram.org/botfake-token-123/getUpdates failed");
        assert!(!out.contains("fake-token-123"));
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn redact_leaves_clean_input_unchanged() {
        let c = test_client();
        assert_eq!(c.redact("connection reset"), "connection reset");
    }

    #[test]
    fn topic_missing_detects_invalid_topic() {
        assert!(topic_missing("Bad Request: TOPIC_ID_INVALID"));
        assert!(!topic_missing("Bad Request: message is not modified"));
        assert!(!topic_missing("connection reset"));
        assert!(topic_not_modified("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(!topic_not_modified("Bad Request: TOPIC_ID_INVALID"));
    }
}
