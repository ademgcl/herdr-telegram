use super::client::TelegramClient;
use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;

pub async fn get_updates(tg: &TelegramClient, offset: u64, poll_secs: i64) -> Res<Vec<Value>> {
    let r = tg
        .call(
            "getUpdates",
            json!({
                "offset": offset,
                "timeout": poll_secs,
                "limit": 100,
                "allowed_updates": ["message", "edited_message", "callback_query", "my_chat_member"],
            }),
            Duration::from_secs(poll_secs as u64 + 10),
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
}
