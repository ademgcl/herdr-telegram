//! Telegram retry policy: flood-wait + transient backoff (split from
//! `client`, 300-line file limit). Single source for the retry budget
//! shared by `call_retrying` and every loud send/edit path.
use super::client::TelegramClient;
use crate::types::Res;
use serde_json::Value;
use std::time::Duration;

impl TelegramClient {
    /// Telegram flood-wait cap: an uncapped `retry after 1000000` would
    /// stall the caller (settle tick, boot menu sync) for days; beyond
    /// the cap the caller fails fast and the next tick retries.
    /// Single source: `call_retrying` + every loud send/edit path share
    /// it, so the cap bounds retry churn everywhere.
    pub(crate) const MAX_FLOOD_WAIT_SECS: u64 = 60;

    pub(crate) fn retry_after(e: &str) -> Option<Duration> {
        let low = e.to_lowercase();
        let (_, tail) = low.split_once("retry after")?;
        // Adjacency only: the digits must follow the marker within an
        // optional ": "/dash run. A bare digit run later in the text
        // ("retry after many (code 123)") is not a flood-wait.
        let it = tail.chars();
        let mut skipped = 0;
        let mut digits = String::new();
        for c in it {
            if digits.is_empty() && (c == ':' || c == ' ' || c == '\t' || c == '-') {
                skipped += 1;
                if skipped > 4 {
                    return None;
                }
                continue;
            }
            if c.is_ascii_digit() {
                digits.push(c);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return None;
        }
        digits
            .parse::<u64>()
            .ok()
            .map(|s| Duration::from_secs(s.saturating_add(1).min(Self::MAX_FLOOD_WAIT_SECS)))
    }

    /// True when a Telegram error string is a transient server/net
    /// fault worth retrying. `call` turns HTTP-200 `ok:false` bodies
    /// into plain string errors, so Telegram 5xx arrives exactly that
    /// way ("Internal Server Error") and a reqwest downcast alone never
    /// matches them — the fatal shapes above already returned early.
    /// Single source: `call_retrying` and the loud send path share it.
    pub(crate) fn is_transient_msg(msg: &str) -> bool {
        let low = msg.to_lowercase();
        [
            "internal server error",
            "bad gateway",
            "service unavailable",
            "gateway timeout",
            "timeout",
            "timed out",
            "connection reset",
            "connection refused",
            "connection closed",
            "network is unreachable",
            "temporary failure",
        ]
        .iter()
        .any(|m| low.contains(m))
    }

    /// Transport half of the retry verdict (single source): only
    /// reqwest transport/server faults arrive typed — Telegram API
    /// fatals arrive as plain strings via `call`, gated by
    /// `is_transient_msg`. Shared by `call_retrying` + the edit path.
    pub(crate) fn is_transport_transient(
        e: &(dyn std::error::Error + Send + Sync + 'static),
    ) -> bool {
        e.downcast_ref::<reqwest::Error>()
            .map(|re| {
                re.is_connect()
                    || re.is_timeout()
                    || re.status().map(|s| s.is_server_error()).unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Execute a Telegram API call with 429 flood-wait (`retry_after`)
    /// and transient connection/server error retries. Fatal permission/topic
    /// errors fail fast without retry.
    pub async fn call_retrying(&self, method: &str, body: Value, timeout: Duration) -> Res<Value> {
        // Single shared budget: flood-wait and transient retries draw
        // from the same pool (≤6 retries total), so the worst case
        // stays bounded instead of 3 floods + 3 transients stacked.
        let mut used = 0;
        loop {
            match self.call(method, body.clone(), timeout).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    if crate::telegram::topic_missing(&msg)
                        || crate::telegram::topic_not_modified(&msg)
                        || msg.contains(crate::telegram::NO_RIGHTS)
                        || msg.contains(crate::telegram::BOT_BLOCKED)
                        || msg.contains("CHAT_ADMIN_REQUIRED")
                    {
                        return Err(e);
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        used += 1;
                        if used > 6 {
                            return Err(e);
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let retryable =
                        Self::is_transport_transient(e.as_ref()) || Self::is_transient_msg(&msg);
                    used += 1;
                    if !retryable || used > 6 {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
}
