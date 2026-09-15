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
}
