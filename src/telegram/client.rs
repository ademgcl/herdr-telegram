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
                    if crate::telegram::topic_missing(&msg)
                        || crate::telegram::topic_not_modified(&msg)
                        || msg.contains("message is not modified")
                        || msg.contains(crate::telegram::NO_RIGHTS)
                        || msg.contains(crate::telegram::BOT_BLOCKED)
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
                    // `call` turns HTTP-200 `ok:false` bodies into plain
                    // string errors, so the reqwest downcast below never
                    // matches them: Telegram 5xx arrives exactly that way
                    // ("Internal Server Error") and must retry, while the
                    // fatal shapes above already returned early.
                    let low = msg.to_lowercase();
                    let server_markers = [
                        "internal server error",
                        "bad gateway",
                        "service unavailable",
                        "gateway timeout",
                        "timed out",
                        "connection reset",
                    ];
                    let retryable = e
                        .downcast_ref::<reqwest::Error>()
                        .map(|re| {
                            re.is_connect()
                                || re.is_timeout()
                                || re.status().map(|s| s.is_server_error()).unwrap_or(false)
                        })
                        .unwrap_or(false)
                        || server_markers.iter().any(|m| low.contains(m));
                    if !retryable || attempts >= 3 {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    /// Global menu table: (command, description). Single source for the
    /// setMyCommands payload and its scope test. Scope tags live in the
    /// descriptions: untagged = responds on every surface (full function
    /// or redirect/refusal guidance), `(topic/DM)` = full function there
    /// with a General redirect, `(forum)`-free: reset is paced-everywhere
    /// with per-topic behavior inside topics (see description).
    pub(crate) const MENU_COMMANDS: &[(&str, &str)] = &[
        ("start", "how to drive agents from here"),
        ("agents", "open control panel (spaces + agents)"),
        ("spawn", "spawn a new agent: /spawn <kind> [space]"),
        ("space", "new space + shell topic"),
        ("model", "current model + free-Zen picker (topic/DM)"),
        ("quit", "drop the agent to a shell (topic/DM)"),
        ("kill", "close the pane completely (topic/DM)"),
        ("shell", "open a fresh shell pane"),
        ("pane", "shell pane that stays here (optional space)"),
        ("split", "sibling shell pane (topic/DM)"),
        (
            "read",
            "recent output: this topic, or focus/reply/sole in DM (topic/DM)",
        ),
        ("output", "alias of /read with line count (topic/DM)"),
        (
            "status",
            "agent card: this topic, or focus/reply/sole in DM (topic/DM)",
        ),
        ("history", "recent prompts you sent (topic/DM)"),
        ("cancel", "abort pending prompts / keys-mode"),
        ("reset", "reset topics (paced all; per-topic inside topics)"),
        ("card", "re-post question + buttons (topic/DM)"),
        ("esc", "guarded Esc dismiss, blocked-only (topic/DM)"),
        (
            "keys",
            "send raw keys: `/keys y enter` here, `/keys <pane> ...` in DM (topic/DM)",
        ),
        ("help", "how to drive agents from here"),
    ];

    pub async fn set_my_commands(&self) -> Res<()> {
        let commands: Vec<Value> = TelegramClient::MENU_COMMANDS
            .iter()
            .map(|(command, description)| json!({"command": command, "description": description}))
            .collect();
        self.call(
            "setMyCommands",
            json!({"commands": commands}),
            Duration::from_secs(15),
        )
        .await?;
        Ok(())
    }

    /// Fire-and-forget menu registration: the menu persists server-side
    /// once set, so a blip at boot must not fail the boot (fail-dead =
    /// launchd crash-loop for the whole outage). Boot continues
    /// immediately; this converges in the background with capped backoff.
    /// A 401/Unauthorized is a dead token, not dead net: log FATAL and
    /// stop (retrying a certain-401 forever only burns log) — but never
    /// exit: that would skip the offset flush and replay recent messages
    /// as duplicate submits on reboot. Fix .env + restart.
    pub fn spawn_menu_sync(self) {
        tokio::spawn(async move {
            let mut wait = 5u64;
            loop {
                match self.set_my_commands().await {
                    Ok(()) => break,
                    Err(e) => {
                        let msg = self.redact(&e.to_string());
                        if msg.contains("401") || msg.contains("Unauthorized") {
                            eprintln!(
                                "[telegram] FATAL: setMyCommands Unauthorized — token invalid, fix .env + restart ({msg})"
                            );
                            break;
                        }
                        // Redact: error text can carry the token in URL form.
                        eprintln!("[telegram] setMyCommands failed, retry in {wait}s: {msg}");
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                        wait = (wait * 2).min(300);
                    }
                }
            }
        });
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

    #[test]
    fn test_menu_names_tags_and_no_alias() {
        // Scope contract (not prose): every menu name, its tag shape, and
        // the deleted alias staying absent. Tags = full-function scope;
        // untagged = responds on every surface (full or guidance).
        // (topic/DM)-tagged: full function there + General redirect.
        let tagged = [
            "model", "quit", "kill", "split", "read", "output", "status", "history", "card", "esc",
            "keys",
        ];
        // Global: full, redirect, or refusal on every surface.
        let global = [
            "start", "agents", "spawn", "space", "shell", "pane", "cancel", "help",
        ];
        let names: Vec<&str> = TelegramClient::MENU_COMMANDS
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(names.len(), tagged.len() + global.len() + 1, "menu grew?");
        for cmd in tagged {
            let desc = TelegramClient::MENU_COMMANDS
                .iter()
                .find(|(c, _)| *c == cmd)
                .unwrap()
                .1;
            assert!(desc.contains("(topic/DM)"), "{cmd} lost its scope tag");
        }
        for cmd in global {
            let desc = TelegramClient::MENU_COMMANDS
                .iter()
                .find(|(c, _)| *c == cmd)
                .unwrap()
                .1;
            assert!(!desc.contains("(topic/DM)"), "{cmd} gained a scope tag");
        }
        let reset = TelegramClient::MENU_COMMANDS
            .iter()
            .find(|(c, _)| *c == "reset")
            .unwrap()
            .1;
        assert!(reset.contains("paced"), "reset lost its paced marker");
        assert!(
            !TelegramClient::MENU_COMMANDS
                .iter()
                .any(|(c, _)| *c == "reset_topics"),
            "deleted alias resurrected in menu"
        );
    }
}
