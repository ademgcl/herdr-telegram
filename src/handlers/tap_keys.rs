use crate::{
    handlers::interactive::{keys_for, opt_keys},
    herdr::client::{get_agent, read_screen_visible, send_agent_keys},
};
use std::time::Duration;

pub(crate) struct TapSend {
    pub(crate) nav: Vec<&'static str>,
    pub(crate) confirm: Vec<&'static str>,
    pub(crate) label: String,
}

pub(crate) enum TapCall {
    Unknown,
    KeysFailed,
    Landed(TapSend, Vec<String>, Vec<String>, bool),
}

/// Send the tap's keys and re-read. Returns what to classify — never
/// touches Telegram itself (the caller owns the card).
pub(crate) async fn tap_keys(socket: &str, pane: &str, action: &str) -> TapCall {
    // Read first: the index is validated against the LIVE dialog below —
    // a stale card (or crafted callback) must never drive keys into a
    // narrower turned-over dialog. Unreadable screens stay permissive:
    // outage must not brick real buttons.
    let before = read_screen_visible(socket, pane, 30).await;
    let (nav, confirm, label): (Vec<&str>, Vec<&str>, String) =
        if let Some(rest) = action.strip_prefix("opt") {
            match rest.parse::<usize>() {
                // opt_keys is the single source of truth; Enter goes separately
                // so the TUI gets a render beat after navigating (else Enter
                // confirms the stale first highlight — the old Tab bug).
                // Cards emit at most 4 options (dialog takes 4): wider
                // indices are crafted callbacks, never real buttons.
                Ok(i) if i < 4 => {
                    if !before.is_empty() {
                        let live = crate::handlers::dialog::parse_options(&before);
                        if !live.is_empty() && i >= live.len() {
                            return TapCall::Unknown;
                        }
                    }
                    let mut full = opt_keys(i);
                    let confirm = vec![full.pop().unwrap()];
                    (full, confirm, format!("option {}", i + 1))
                }
                _ => return TapCall::Unknown,
            }
        } else {
            match keys_for(action) {
                Some(keys) => (Vec::new(), keys.to_vec(), action.to_string()),
                None => return TapCall::Unknown,
            }
        };
    if !nav.is_empty() {
        if send_agent_keys(socket, pane, &nav).await.is_err() {
            return TapCall::KeysFailed;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    if send_agent_keys(socket, pane, &confirm).await.is_err() {
        return TapCall::KeysFailed;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = read_screen_visible(socket, pane, 30).await;
    // Ground truth for "resumed": fresh status beats screen heuristics
    // (a working screen full of prose is not a dialog, and a lagging
    // status flip is covered by the observation layer's sig check).
    let still_blocked = get_agent(socket, pane)
        .await
        .map(|a| a.status == "blocked")
        .unwrap_or(true);
    TapCall::Landed(
        TapSend {
            nav,
            confirm,
            label,
        },
        before,
        after,
        still_blocked,
    )
}
