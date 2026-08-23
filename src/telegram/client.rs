use std::time::Duration;
use serde_json::{json, Value};
use crate::{types::Res, ui::fit_msg};

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

    pub async fn call(&self, method: &str, body: Value, timeout: Duration) -> Res<Value> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        let resp = self.http.post(&url).json(&body).timeout(timeout).send().await?;
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
            match self.call("sendMessage", params.clone(), Duration::from_secs(15)).await {
                Ok(v) => return v["message_id"].as_i64(),
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("sendMessage failed (attempt {}): {msg}", attempt + 1);
                    if let Some(wait) = Self::retry_after(&msg).filter(|w| w.as_secs() <= 60) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| re.is_connect())
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
        let body = fit_msg(text);
        let mut params = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": body,
        });
        if let Some(kb) = keyboard {
            params["reply_markup"] = json!({"inline_keyboard": kb});
        }
        if let Err(e) = self.call("editMessageText", params, Duration::from_secs(15)).await {
            let msg = e.to_string();
            if !msg.contains("message is not modified") {
                eprintln!("editMessageText failed: {msg}");
            }
        }
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

    pub async fn edit_forum_topic(&self, chat_id: i64, thread_id: i64, name: &str) -> Res<()> {
        self.call(
            "editForumTopic",
            json!({
                "chat_id": chat_id,
                "message_thread_id": thread_id,
                "name": name,
            }),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
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

    pub async fn set_my_commands(&self) -> Res<()> {
        let _ = self
            .call(
                "setMyCommands",
                json!({"commands": [
                    {"command": "agents", "description": "open control panel (spaces + agents)"},
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
