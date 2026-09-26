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
            Duration::from_secs(secs.saturating_add(10)),
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
/// overrides the failure backoff when within the sleep cap — re-hitting
/// before it expires only extends the flood (and the 5–30s cap above
/// would do exactly that). Over-cap waits fail fast to the normal
/// backoff (next tick retries — never stall the poll loop for days).
/// Single source for the poll-error arm in `main`.
pub(crate) fn flood_aware_poll_wait(err_msg: &str, fails: u32) -> u64 {
    let backoff = poll_backoff_secs(fails);
    match super::client::TelegramClient::retry_after(err_msg) {
        Some(d) if !super::client::TelegramClient::flood_wait_exceeds_cap(d) => {
            d.as_secs().max(backoff)
        }
        _ => backoff,
    }
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
        // Flood-wait within the cap overrides the short backoff (never re-hit early).
        assert_eq!(
            flood_aware_poll_wait("Too Many Requests: retry after 45", 1),
            46
        );
        // No flood marker: plain backoff.
        assert_eq!(flood_aware_poll_wait("connection reset", 2), 10);
        // Over-cap wait fails fast to the normal backoff — never a multi-day stall.
        assert_eq!(
            flood_aware_poll_wait("Too Many Requests: retry after 1000000", 6),
            30
        );
    }
}
