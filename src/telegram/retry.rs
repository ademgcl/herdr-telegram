//! Telegram retry policy: flood-wait + transient backoff (split from
//! `client`, 300-line file limit). Single source for the retry budget
//! shared by `call_retrying` and every loud send/edit path.
use super::client::TelegramClient;
use crate::types::Res;
use serde_json::Value;
use std::time::Duration;

impl TelegramClient {
    /// Telegram flood-wait sleep cap: an uncapped `retry after 1000000`
    /// would stall the caller (settle tick, boot menu sync) for days;
    /// beyond the cap the caller fails fast (`flood_wait_exceeds_cap`)
    /// and the next tick retries.
    /// Single source: `call_retrying` + every loud send/edit path share
    /// it, so the cap bounds retry churn everywhere.
    pub(crate) const MAX_FLOOD_WAIT_SECS: u64 = 60;

    /// True when a parsed flood-wait exceeds the sleep cap: callers must
    /// fail fast (next tick retries) instead of stalling for days.
    /// Pure for tests.
    pub(crate) fn flood_wait_exceeds_cap(wait: Duration) -> bool {
        wait.as_secs() > Self::MAX_FLOOD_WAIT_SECS
    }

    pub(crate) fn retry_after(e: &str) -> Option<Duration> {
        // Normalize first: Telegram ships `retry after N`, `retry_after N`
        // and bare `FLOOD_WAIT_N` shapes — the underscore/bare forms must
        // honor the same flood-wait instead of failing fast + dropping.
        let low = e.to_lowercase().replace('_', " ");
        if let Some((_, tail)) = low.split_once("retry after") {
            return Self::retry_after_tail(tail);
        }
        // Bare FLOOD_WAIT_30 (underscores already normalized to spaces).
        if let Some(idx) = low.find("flood wait") {
            return Self::retry_after_tail(&low[idx + "flood wait".len()..]);
        }
        None
    }

    fn retry_after_tail(tail: &str) -> Option<Duration> {
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
        // Uncapped: callers gate on `flood_wait_exceeds_cap` and fail
        // fast past MAX_FLOOD_WAIT_SECS (cap-in-parse made every huge
        // wait look sleepable and stalled a tick for the full cap).
        digits
            .parse::<u64>()
            .ok()
            .map(|s| Duration::from_secs(s.saturating_add(1)))
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
            // A 429 without a parsable `retry after N` (reworded flood
            // text, em-dash separator, missing number) must retry with
            // backoff, never fail fast and silently drop the buzz.
            "too many requests",
            "flood",
        ]
        .iter()
        .any(|m| low.contains(m))
    }

    /// Fatal permission/topic errors fail fast without retry.
    /// Case-insensitive (errors.rs parity): Telegram ships sentence-case
    /// variants too, and a missed fatal burns 6 retries + sleeps instead
    /// of failing fast. Pure for tests.
    pub(crate) fn is_fatal_msg(msg: &str) -> bool {
        let low = msg.to_lowercase();
        crate::telegram::topic_missing(msg)
            || crate::telegram::topic_not_modified(msg)
            || low.contains(crate::telegram::NO_RIGHTS)
            || low.contains(crate::telegram::BOT_BLOCKED)
            || low.contains("chat_admin_required")
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
                    if Self::is_fatal_msg(&msg) {
                        return Err(e);
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        // Over-cap flood: fail fast — the next settle tick
                        // retries; sleeping the raw wait stalls for days.
                        if Self::flood_wait_exceeds_cap(wait) {
                            return Err(e);
                        }
                        used += 1;
                        if used > 6 {
                            return Err(e);
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let retryable = Self::is_transient_msg(&msg);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fatal_match_is_case_insensitive() {
        // Sentence-case variants must fail fast, not burn 6 retries.
        assert!(TelegramClient::is_fatal_msg(
            "Forbidden: Not Enough Rights to send text messages"
        ));
        assert!(TelegramClient::is_fatal_msg(
            "Forbidden: Bot Was Blocked by the user"
        ));
        assert!(TelegramClient::is_fatal_msg(
            "Bad Request: CHAT_ADMIN_REQUIRED"
        ));
        assert!(TelegramClient::is_fatal_msg(
            "Bad Request: Chat_admin_required"
        ));
        assert!(!TelegramClient::is_fatal_msg("Internal Server Error"));
        assert!(!TelegramClient::is_fatal_msg("connection reset"));
    }

    #[test]
    fn test_flood_wait_exceeds_cap() {
        use std::time::Duration;
        // Within cap: sleep and retry.
        assert!(!TelegramClient::flood_wait_exceeds_cap(
            Duration::from_secs(60)
        ));
        assert!(!TelegramClient::flood_wait_exceeds_cap(
            Duration::from_secs(31)
        ));
        // Beyond cap: fail fast (next tick retries) — never sleep days.
        assert!(TelegramClient::flood_wait_exceeds_cap(Duration::from_secs(
            61
        )));
        assert!(TelegramClient::flood_wait_exceeds_cap(Duration::from_secs(
            1_000_001
        )));
        assert!(TelegramClient::flood_wait_exceeds_cap(Duration::from_secs(
            u64::MAX
        )));
    }
}
