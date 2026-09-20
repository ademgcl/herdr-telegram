//! Shared esc acks: corpse-vs-outage probe, not-blocked refuse,
//! get_agent error classify, and the stripped-card heal. Split from
//! `escape` (300-line file limit); single source for post_card +
//! esc_pane (+ shell Esc corpse parity).
use crate::state::AppState;
use std::time::Duration;

/// Corpse-vs-outage probe (dm_info parity): confirmed-gone reports
/// UNKNOWN_TARGET, blips retry. Single source for post_card + esc_pane.
pub(crate) async fn gone_ack(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    match crate::herdr::client::list_panes(&s.cfg.socket).await {
        Ok(l) if l.contains(&pane.to_string()) => {
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
        }
        Ok(_) => {
            s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_TARGET, None)
                .await;
        }
        Err(_) => {
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
        }
    }
}

/// Shared not-blocked ack (single source for pre + post gates):
/// remedy syntax differs by surface (topics own-pane, DM names pane).
pub(crate) async fn not_blocked_msg(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    status: &str,
) {
    let hint = match thread {
        Some(_) => "`/keys esc`".to_string(),
        None => format!("`/keys {pane} esc`"),
    };
    s.tg.send_msg(
        chat,
        thread,
        &crate::ui::not_blocked_ack(status, Some(&hint)),
        None,
    )
    .await;
}

/// Heal delay for a stripped card: buttons came off optimistically —
/// re-render after this, instead of stranding buttonless until the
/// ≤60s watchdog. Pinned below (must stay well under the watchdog).
pub const HEAL_DELAY_SECS: u64 = 5;

/// Heal a stripped card: buttons came off optimistically (strip
/// before slow RPC) — re-render after 5s instead of stranding
/// buttonless until the ≤60s watchdog. Single source for every
/// post-strip arm; safe when resumed (refresh resolves, never posts).
pub(crate) fn heal_stripped(s: &AppState, pane: &str) {
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(HEAL_DELAY_SECS)).await;
        crate::handlers::dialog::refresh_blocked_card(&s2, &pane2).await;
    });
}

/// Shared get_agent error ack (single source for all gates): classified
/// death → gone probe, blips → retryable. Never UNKNOWN_TARGET on outage.
/// Verdict via esc_guard::classify (pinned by test).
pub(crate) async fn get_err_msg(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    msg: &str,
) {
    match super::esc_guard::classify(Err(msg.to_string())) {
        super::esc_guard::EscGate::Gone => gone_ack(s, chat, thread, pane).await,
        _ => {
            _ =
                s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                    .await
        }
    }
}

#[cfg(test)]
#[path = "esc_ack_tests.rs"]
mod tests;
