//! Never-stuck escapes for blocked dialogs: `/card` re-posts the live
//! question + buttons (read-only, always safe), `/esc` sends a guarded
//! Escape (blocked-status only, never into live work). Commands — not
//! buttons — are the out-of-band path, so they work even when every
//! card on screen is stale or gone.
use super::target::resolve_target;
use crate::{
    handlers::dialog::send_blocked_card,
    handlers::tap_classify::{TapResult, classify_tap},
    herdr::client::{get_agent, read_screen_visible, send_agent_keys, send_pane_keys},
    state::{AppState, OpGuard},
    types::AgentRow,
};
use std::time::Duration;

/// Re-post the pane's live dialog card. Read-only: never sends keys,
/// never clears waiters, refuses while a tap owns the pane.
async fn post_card(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    if s.blockop.lock().await.contains(pane) {
        s.tg.send_msg(
            chat,
            thread,
            "answer in flight — wait a beat, then /card",
            None,
        )
        .await;
        return;
    }
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status == "blocked" => {
            if !send_blocked_card(s, chat, thread, pane).await {
                s.tg.send_msg(
                    chat,
                    thread,
                    "⚠️ card failed — try /read, or answer on PC",
                    None,
                )
                .await;
            }
        }
        Ok(a) => {
            s.tg.send_msg(
                chat,
                thread,
                &format!("not blocked (status={}) — nothing to answer", a.status),
                None,
            )
            .await;
        }
        Err(_) => {
            s.tg.send_msg(chat, thread, "⚠️ herdr unreachable — try again", None)
                .await;
        }
    }
}

/// Guarded dismiss: Escape only while verifiably blocked, then verify
/// on screen like a tap (resume / new dialog / still blocked).
async fn esc_pane(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    if s.blockop.lock().await.contains(pane) {
        s.tg.send_msg(
            chat,
            thread,
            "answer in flight — wait a beat, then /esc",
            None,
        )
        .await;
        return;
    }
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status == "blocked" => {}
        Ok(a) => {
            s.tg
                .send_msg(
                    chat,
                    thread,
                    &format!(
                        "not blocked (status={}) — Esc would hit live work; use /keys esc if you really mean it",
                        a.status
                    ),
                    None,
                )
                .await;
            return;
        }
        Err(_) => {
            s.tg.send_msg(chat, thread, "⚠️ herdr unreachable — try again", None)
                .await;
            return;
        }
    }
    let Some(_op) = OpGuard::claim(&s.blockop, pane).await else {
        s.tg.send_msg(
            chat,
            thread,
            "answer in flight — wait a beat, then /esc",
            None,
        )
        .await;
        return;
    };
    let before = read_screen_visible(&s.cfg.socket, pane, 30).await;
    if send_agent_keys(&s.cfg.socket, pane, &["esc"])
        .await
        .is_err()
    {
        s.tg.send_msg(chat, thread, "⚠️ keys failed — answer on the PC", None)
            .await;
        return;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = read_screen_visible(&s.cfg.socket, pane, 30).await;
    let still_blocked = get_agent(&s.cfg.socket, pane)
        .await
        .map(|a| a.status == "blocked")
        .unwrap_or(true);
    match classify_tap(&before, &after, still_blocked) {
        TapResult::Resumed => {
            s.blocked_sig.lock().await.remove(pane);
            let mid = s
                .tg
                .send_msg(chat, thread, "✅ dismissed — agent resumed", None)
                .await;
            if let Some(m) = mid {
                let _ = s.tg.set_reaction(chat, m, Some("✅")).await;
            }
        }
        TapResult::NewDialog => {
            s.tg.send_msg(
                chat,
                thread,
                "🚫 dismissed — it asked something else:",
                None,
            )
            .await;
            post_card(s, chat, thread, pane).await;
        }
        TapResult::Unchanged => {
            s.tg.send_msg(
                chat,
                thread,
                "still blocked — /card for fresh buttons, or /kill",
                None,
            )
            .await;
        }
    }
}

/// DM target order matches /read: explicit arg, else replied-to card,
/// else live focus, else sole agent. Fails closed, never guesses.
async fn resolve_dm_pane(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) -> Option<String> {
    let mut row = match resolve_target(rows, if arg.is_empty() { None } else { Some(arg) }) {
        Some(r) => Some(r),
        None if !arg.is_empty() => {
            s.tg.send_msg(chat, None, "unknown target — see /agents", None)
                .await;
            return None;
        }
        None => reply_pane
            .as_deref()
            .and_then(|p| rows.iter().find(|r| r.pane == p).cloned()),
    };
    if row.is_none() {
        if let Some(f) = s
            .get_focus()
            .await
            .filter(|f| rows.iter().any(|r| &r.pane == f))
        {
            row = rows.iter().find(|r| r.pane == f).cloned();
        } else {
            row = resolve_target(rows, Some(""));
        }
    }
    match row {
        Some(r) => Some(r.pane),
        None => {
            s.tg.send_msg(chat, None, "unknown target — see /agents", None)
                .await;
            None
        }
    }
}

pub async fn handle_card_topic(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    post_card(s, chat, thread, pane).await;
}

pub async fn handle_esc_topic(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    esc_pane(s, chat, thread, pane).await;
}

pub async fn handle_card_dm(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    if let Some(pane) = resolve_dm_pane(s, chat, rows, arg, reply_pane).await {
        post_card(s, chat, None, &pane).await;
    }
}

pub async fn handle_esc_dm(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    if let Some(pane) = resolve_dm_pane(s, chat, rows, arg, reply_pane).await {
        esc_pane(s, chat, None, &pane).await;
    }
}

/// Shell topics hold no blocked dialogs: Esc goes raw with a vim
/// warning, and /card has nothing to re-post.
pub async fn handle_esc_shell(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    match send_pane_keys(&s.cfg.socket, pane, &["esc"]).await {
        Ok(_) => {
            s.tg.send_msg(chat, thread, "⌨️ sent Esc (in vim this toggles mode)", None)
                .await;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
        }
    }
}
