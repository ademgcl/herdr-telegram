use crate::types::Res;
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

    pub async fn get_me(&self) -> Res<Value> {
        self.call_retrying("getMe", json!({}), Duration::from_secs(15))
            .await
    }

    /// Telegram flood-wait: "Too Many Requests: retry after N" — honor it
    /// instead of silently dropping the message.
    pub(crate) fn retry_after(e: &str) -> Option<Duration> {
        let tail = e.rsplit("retry after").next()?.trim();
        tail.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<u64>()
            .ok()
            .map(|s| Duration::from_secs(s + 1))
    }

    /// Execute a Telegram API call with 429 flood-wait (`retry_after`)
    /// and transient connection/server error retries. Fatal permission/topic
    /// errors fail fast without retry.
    pub async fn call_retrying(&self, method: &str, body: Value, timeout: Duration) -> Res<Value> {
        let mut attempts = 0;
        let mut waits = 0;
        loop {
            match self.call(method, body.clone(), timeout).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    if super::errors::topic_missing(&msg)
                        || super::errors::topic_not_modified(&msg)
                        || msg.contains("message is not modified")
                        || msg.contains("not enough rights")
                        || msg.contains("bot was blocked")
                        || msg.contains("CHAT_ADMIN_REQUIRED")
                    {
                        return Err(e);
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 3 {
                            return Err(e);
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    attempts += 1;
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| {
                            re.is_connect()
                                || re.is_timeout()
                                || re.status().map(|s| s.is_server_error()).unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if !retryable || attempts >= 3 {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    pub async fn set_my_commands(&self) -> Res<()> {
        self.call(
            "setMyCommands",
            json!({"commands": [
                // Topic-scoped commands (/split, ...) stay out of the
                // global menu: they only work inside a pane topic.
                {"command": "start", "description": "how to drive agents from here"},
                {"command": "agents", "description": "open control panel (spaces + agents)"},
                {"command": "spawn", "description": "spawn a new agent: /spawn <kind> [space]"},
                {"command": "space", "description": "new space + shell topic"},
                {"command": "model", "description": "current model + free-Zen picker"},
                {"command": "quit", "description": "drop the agent to a shell"},
                {"command": "kill", "description": "close the pane completely"},
                {"command": "shell", "description": "open a fresh shell pane"},
                {"command": "read", "description": "recent output of focused agent"},
                {"command": "output", "description": "alias of /read with line count"},
                {"command": "status", "description": "agent card for focused agent"},
                {"command": "cancel", "description": "abort pending prompts / keys-mode"},
                {"command": "reset", "description": "paced reset of all topics (forum)"},
                {"command": "card", "description": "re-post question + buttons (topic/DM)"},
                {"command": "esc", "description": "guarded Esc dismiss, blocked-only"},
                {"command": "keys", "description": "/keys <pane> y enter — send raw keys"},
                {"command": "help", "description": "how to drive agents from here"},
            ]}),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
    }
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
    fn test_redact_leaves_clean_input_unchanged() {
        let c = test_client();
        assert_eq!(c.redact("connection reset"), "connection reset");
    }

    #[test]
    fn test_retry_after_parsing() {
        use std::time::Duration;
        // +1s bias on top of the asked wait.
        assert_eq!(
            TelegramClient::retry_after("Too Many Requests: retry after 30"),
            Some(Duration::from_secs(31))
        );
        assert_eq!(
            TelegramClient::retry_after("retry after 0"),
            Some(Duration::from_secs(1))
        );
        assert_eq!(TelegramClient::retry_after("connection reset"), None);
        assert_eq!(TelegramClient::retry_after("retry after many"), None);
    }
}
