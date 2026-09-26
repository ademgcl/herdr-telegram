//! Callback waiter arms (K/R) + output taps (p/o): split from `callback`
//! (300-line file limit).
//! Exclusive arming: a sibling waiter for the same key would otherwise
//! win the next message instead of the tapped one.
use super::callback_parse::{gone_card, pane_live};
use crate::{
    herdr::client::{
        get_agent, list_panes, list_workspaces, read_agent_output, read_agent_visible,
        read_shell_output,
    },
    state::AppState,
    ui::{pane_output_kb, ws_label},
};
use std::time::Instant;

/// `K:<pane>`: next message types as raw keys into the pane.
/// Fail-closed on outage like the R arm: an unreadable herdr never arms
/// a waiter, moves focus, or edits the card (ambiguous read → no write).
/// Confirmed-dead panes retire the stale card; confirmed shells proceed.
pub(crate) async fn handle_keys_arm(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
) {
    if get_agent(&s.cfg.socket, pane).await.is_err() {
        match list_panes(&s.cfg.socket).await {
            Ok(l) if l.contains(&pane.to_string()) => {}
            Ok(_) => {
                gone_card(s, chat, msg_id, pane).await;
                return;
            }
            Err(_) => {
                // Notice keeps the card keyboard (`edit_msg(None)` drops it).
                s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return;
            }
        }
    }
    s.runwait
        .lock()
        .await
        .remove(&super::forum::waiter_key(chat, thread));
    s.typewait
        .lock()
        .await
        .remove(&super::forum::waiter_key(chat, thread));
    s.keywait.lock().await.insert(
        super::forum::waiter_key(chat, thread),
        (pane.to_string(), Instant::now()),
    );
    // No arm-time focus: focus follows successful SENDS only (an
    // abandoned or expired waiter must not pin routing at arm time).
    s.tg.edit_msg(
        chat,
        msg_id,
        &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"),
        None,
    )
    .await;
}

/// `R:<ws>`: next message runs as a shell command in the workspace.
/// Fail-closed on outage: an unreadable list never reads as "gone".
pub(crate) async fn handle_run_arm(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    ws: &str,
) {
    let spaces = match list_workspaces(&s.cfg.socket).await {
        Ok(sp) => sp,
        Err(_) => {
            // Notice keeps the card keyboard (`edit_msg(None)` drops it).
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return;
        }
    };
    if !spaces.iter().any(|w| w.id == *ws) {
        s.tg.edit_msg(chat, msg_id, &format!("space {ws} is gone"), None)
            .await;
        s.forget_target(chat, msg_id).await;
        return;
    }
    s.keywait
        .lock()
        .await
        .remove(&super::forum::waiter_key(chat, thread));
    s.typewait
        .lock()
        .await
        .remove(&super::forum::waiter_key(chat, thread));
    s.runwait.lock().await.insert(
        super::forum::waiter_key(chat, thread),
        (ws.to_string(), Instant::now()),
    );
    let label = ws_label(&spaces, ws);
    s.tg.edit_msg(
        chat,
        msg_id,
        &format!("⌨️ send shell command for {label}\nnext message = command"),
        None,
    )
    .await;
}

/// Fail-closed output read: a herdr outage must not edit "pane gone"
/// (which also forgets routing — the next reply would misroute into
/// focus/sole-agent). Unconfirmed-dead refuses visibly instead. Shared
/// by the p/o arms (pane output vs agent output differ only in source).
async fn read_output_or_gate(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
    agent: bool,
) -> Option<String> {
    let res = if agent {
        // Blocked/working alternate-screen panes reject recent_unwrapped
        // (topic_read parity): fall back to visible, except confirmed
        // death (wrong-pane output) and timeouts (doubled RPC budget).
        match read_agent_output(&s.cfg.socket, pane, 120).await {
            Ok(out) => Ok(out),
            Err(e) if crate::herdr::rpc::should_fallback_visible(&e.to_string()) => {
                read_agent_visible(&s.cfg.socket, pane, 120).await
            }
            Err(e) => Err(e),
        }
    } else {
        // Pane arm serves shells/alt-screen panes: `read_shell_output`
        // is the single source for recent_unwrapped→visible (topic_read
        // parity) with death/timeout guards — a raw pane.read here would
        // edit HERDR_UNREACHABLE where /read succeeds.
        read_shell_output(&s.cfg.socket, pane, 120).await
    };
    match res {
        Ok(out) => Some(out),
        Err(_) => {
            if pane_live(s, pane).await {
                // Notice keeps the output-card keyboard (`edit_msg(None)` drops it).
                s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
            } else {
                gone_card(s, chat, msg_id, pane).await;
            }
            None
        }
    }
}

/// `p:<pane>`: post the pane's visible output with a refresh button.
pub(crate) async fn handle_pane_output(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
) {
    let Some(out) = read_output_or_gate(s, chat, msg_id, thread, pane, false).await else {
        return;
    };
    // A read can succeed for a just-deleted pane: tracking it would
    // poison routing (next reply misroutes via `targets`). Retire the
    // stale card instead, like the B/M/X arms.
    if !pane_live(s, pane).await {
        gone_card(s, chat, msg_id, pane).await;
        return;
    }
    let body = if out.is_empty() {
        crate::ui::NO_OUTPUT.into()
    } else {
        out
    };
    let mid =
        s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(pane)))
            .await;
    s.remember(chat, mid, pane).await;
}

/// `o:<pane>`: post the agent's output with a back button.
pub(crate) async fn handle_agent_output(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
) {
    let Some(out) = read_output_or_gate(s, chat, msg_id, thread, pane, true).await else {
        return;
    };
    if !pane_live(s, pane).await {
        gone_card(s, chat, msg_id, pane).await;
        return;
    }
    let body = if out.is_empty() {
        crate::ui::NO_OUTPUT.into()
    } else {
        out
    };
    let kb = serde_json::json!([[crate::ui::btn("← back", &format!("a:{pane}"))]]);
    let mid = s.tg.send_msg(chat, thread, &body, Some(kb)).await;
    s.remember(chat, mid, pane).await;
}
