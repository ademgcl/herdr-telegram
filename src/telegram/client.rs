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
        if self.token.is_empty() {
            return s.to_string();
        }
        s.replace(&self.token, "<redacted>")
    }

    pub async fn call(&self, method: &str, body: Value, timeout: Duration) -> Res<Value> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.token, method);
        // Redact at the source: reqwest::Error's Display embeds the
        // request URL including the bot token — returning it raw leaks
        // the token to every caller that logs the error.
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| {
                // `Res` erases the reqwest type, so the transport verdict
                // (`is_transport_transient` downcast) can never fire on
                // send-stage errors — and `e.to_string()` shows only the
                // outer "error sending request" line, hiding the inner
                // timeout/connect cause from `is_transient_msg`. Tag
                // transport faults here so the retry loop sees them.
                let transient = e.is_connect()
                    || e.is_timeout()
                    || e.status().map(|s| s.is_server_error()).unwrap_or(false);
                let m = self.redact(&e.to_string());
                if transient {
                    format!("service unavailable (transient transport): {m}")
                } else {
                    m
                }
            })?;
        let status = resp.status();
        // `.timeout()` above covers `.send()` (headers) only — a stalled
        // body would hang the poll/watchdog loop inside `json()`. Bound
        // the read with the same budget instead of hanging forever.
        // Read as text first: proxy/CF 502 HTML bodies are not JSON —
        // a straight `resp.json()` turns them into a reqwest decode
        // error that matches neither the transient set nor the
        // token-death check (zero retries / missed FATAL). Map decode
        // failures and 5xx statuses to a transient string instead.
        let text = tokio::time::timeout(timeout, resp.text())
            .await
            .map_err(|_| "telegram response timed out")?
            .map_err(|e| {
                let m = self.redact(&e.to_string());
                format!("telegram http {status}: service unavailable ({m})")
            })?;
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            let snippet: String = text.chars().take(120).collect();
            let m = self.redact(&snippet);
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                format!("telegram http {status}: service unavailable ({m} {e})")
            } else {
                // Preserve token-death markers ("Unauthorized",
                // "Not Found") when the body carries them so
                // `is_unauthorized` FATALs instead of transient-retrying
                // a dead token; otherwise still retryable once via the
                // transient marker.
                let low = m.to_lowercase();
                if low.contains("unauthorized") || low.contains("not found") {
                    format!("telegram http {status}: {m} ({e})")
                } else {
                    format!("telegram http {status}: service unavailable ({m} {e})")
                }
            }
        })?;
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

    /// Best-effort message delete (working-card retire after the final
    /// lands): true when gone. Single attempt, failures stay silent —
    /// the caller falls back to folding the card in place, so a lost
    /// delete right never strands a frozen "working…" corpse.
    pub async fn delete_msg(&self, chat_id: i64, message_id: i64) -> bool {
        match self
            .call(
                "deleteMessage",
                json!({"chat_id": chat_id, "message_id": message_id}),
                Duration::from_secs(10),
            )
            .await
        {
            Ok(_) => true,
            Err(e) => {
                eprintln!("deleteMessage failed: {}", self.redact(&e.to_string()));
                false
            }
        }
    }

    /// True when a Telegram error means the token is dead (revoked/
    /// invalid), never a transient fault. Matches the `Unauthorized`
    /// description Telegram sends for bad tokens and the bare `Not Found`
    /// body it sends for a deleted/revoked token (HTTP 404). The bare
    /// match is deliberate: contextual "…not found" (message/thread/chat
    /// corpses) must never FATAL-exit a healthy daemon into a stop.
    pub(crate) fn is_unauthorized(msg: &str) -> bool {
        msg.contains("Unauthorized")
            || msg.contains("unauthorized")
            || msg.trim() == "Not Found"
            || msg.contains(": Not Found")
            // Non-JSON bare-404 bodies decode-fail inside `call` and get
            // wrapped as `telegram http 404: Not Found (…)` — the colon
            // sits AFTER `Not Found`, so neither arm above fires and token
            // death reads as transient (backoff forever, squatting the
            // single-instance guard). The `Not Found` marker is required:
            // a bare proxy-404 wrapper (`telegram http 404: service
            // unavailable (…)`) is a blip, never token death.
            || (msg.contains("http 404") && msg.to_lowercase().contains("not found"))
    }
    /// Fire-and-forget menu registration: the menu persists server-side
    /// once set, so a blip at boot must not fail the boot (fail-dead =
    /// launchd crash-loop for the whole outage). Boot continues
    /// immediately; this converges in the background with capped backoff.
    /// A 401/Unauthorized or 404/Not Found is a dead token, not dead
    /// net: log FATAL and stop (retrying a certain-death forever only
    /// burns log) — but never exit: that would skip the offset flush
    /// and replay recent messages as duplicate submits on reboot.
    /// Fix .env + restart.
    pub fn spawn_menu_sync(self) {
        tokio::spawn(async move {
            let mut wait = 5u64;
            loop {
                match self.set_my_commands().await {
                    Ok(()) => break,
                    Err(e) => {
                        let msg = self.redact(&e.to_string());
                        if Self::is_unauthorized(&msg) {
                            eprintln!(
                                "[telegram] FATAL: setMyCommands unauthorized/not-found — token invalid, fix .env + restart ({msg})"
                            );
                            break;
                        }
                        // Redact: error text can carry the token in URL form.
                        eprintln!("[telegram] setMyCommands failed, retry in {wait}s: {msg}");
                        // Honor flood-wait: Telegram's `retry after N` can
                        // exceed the fixed backoff — re-hitting early only
                        // extends the flood.
                        let pause = Self::retry_after(&msg)
                            .map(|d| d.max(Duration::from_secs(wait)))
                            .unwrap_or_else(|| Duration::from_secs(wait));
                        tokio::time::sleep(pause).await;
                        wait = (wait * 2).min(300);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
