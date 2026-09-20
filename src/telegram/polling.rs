use super::client::TelegramClient;
use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;

pub async fn get_updates(tg: &TelegramClient, offset: u64, poll_secs: i64) -> Res<Vec<Value>> {
    // edited_message updates are always dropped by the router
    // (resend-as-new by design) — don't subscribe to the payload.
    let secs = poll_secs.max(0) as u64;
    let r = tg
        .call(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": secs,
                "limit": 100,
                "allowed_updates": ["message", "callback_query", "my_chat_member"],
            }),
            Duration::from_secs(secs + 10),
        )
        .await?;
    Ok(r.as_array().cloned().unwrap_or_default())
}

/// Backoff after consecutive poll failures: instant failures (DNS down)
/// must not hot-loop the log every 5s, but recovery must stay prompt.
/// Pure for tests.
pub(crate) fn poll_backoff_secs(fails: u32) -> u64 {
    (5 * u64::from(fails.max(1))).min(30)
}

/// Flood-aware poll wait (pure, tested): Telegram's `retry after N`
/// overrides the failure backoff — re-hitting before it expires only
/// extends the flood (and the 5–30s cap above would do exactly that).
/// Single source for the poll-error arm in `main`.
pub(crate) fn flood_aware_poll_wait(err_msg: &str, fails: u32) -> u64 {
    let backoff = poll_backoff_secs(fails);
    super::client::TelegramClient::retry_after(err_msg)
        .map(|d| d.as_secs().max(backoff))
        .unwrap_or(backoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_backoff_ramps_then_caps() {
        assert_eq!(poll_backoff_secs(0), 5);
        assert_eq!(poll_backoff_secs(1), 5);
        assert_eq!(poll_backoff_secs(2), 10);
        assert_eq!(poll_backoff_secs(5), 25);
        assert_eq!(poll_backoff_secs(6), 30);
        assert_eq!(poll_backoff_secs(100), 30);
    }

    #[test]
    fn test_flood_aware_poll_wait_honors_retry_after() {
        // Flood-wait overrides the short backoff (never re-hit early).
        assert_eq!(
            flood_aware_poll_wait("Too Many Requests: retry after 45", 1),
            46
        );
        // No flood marker: plain backoff.
        assert_eq!(flood_aware_poll_wait("connection reset", 2), 10);
        // Flood-wait capped at 60s+1 by retry_after, still wins over 30s cap.
        assert_eq!(
            flood_aware_poll_wait("Too Many Requests: retry after 1000000", 6),
            60
        );
    }
}
