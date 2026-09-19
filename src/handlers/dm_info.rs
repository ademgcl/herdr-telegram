use super::target::{resolve_target, unmatched_reply};
use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output, send_agent_keys},
    state::AppState,
    types::AgentRow,
    ui::{
        agent_card_kb, build_agent_card_text,
        scope_text::{READ_CAP, TOPIC_READ_DEFAULT},
        ws_label,
    },
};

pub(crate) async fn handle_keys(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    let (t, keys) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
    // Explicit `<pane|kind> <keys...>`, else the full arg is keys for the
    // replied-to card. A stale reply never silently reroutes to a
    // different agent: shell/dead panes fail visibly inside send_keys.
    // An explicit pane with no keys is a usage error, not keys for the
    // reply (typo guard) — as is a kind/pane-shaped word that matched
    // nothing (ambiguous kinds must not become keystrokes elsewhere).
    let (pane, keys) = match resolve_target(rows, Some(t)) {
        Some(r) if !keys.is_empty() => (Some(r.pane), keys),
        Some(_) => (None, keys),
        None if rows.iter().any(|r| r.kind == t) || t.contains(':') => (None, keys),
        None if reply_pane.is_some() && !arg.is_empty() => (reply_pane.clone(), arg),
        _ => (None, keys),
    };
    let Some(pane) = pane else {
        s.tg.send_msg(
            chat,
            None,
            "usage: /keys <pane|kind> <key> [key...]  e.g. /keys w8:p1 y enter",
            None,
        )
        .await;
        return;
    };
    send_keys(s, chat, &pane, keys).await;
}

async fn send_keys(s: &AppState, chat: i64, pane: &str, keys: &str) {
    // Never interleave with an owned key sequence (mirrors topic /keys).
    // Self-healing peeks: stale corpses evict instead of blocking.
    if s.block_held(pane).await || s.model_held(pane).await {
        s.tg.send_msg(chat, None, crate::ui::TAP_MODEL_IN_FLIGHT, None)
            .await;
        return;
    }
    let key_list: Vec<&str> = keys.split_whitespace().collect();
    // Bounded like every keys arm (single source): refuse, never truncate.
    if let Err(msg) = super::shell_validate::validate_keys_len(key_list.len()) {
        s.tg.send_msg(chat, None, &msg, None).await;
        return;
    }
    match send_agent_keys(&s.cfg.socket, pane, &key_list).await {
        Ok(_) => {
            s.tg.send_msg(chat, None, crate::ui::KEYS_SENT, None).await;
        }
        Err(e) => {
            // Masked (herdr errors carry socket/cwd paths — reason kept,
            // username dropped).
            s.tg.send_msg(chat, None, &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None).await;
        }
    }
}

pub(crate) async fn handle_read(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    // Target order matches /status: explicit arg, else replied-to card,
    // else live focus, else the sole agent. An explicit but unknown arg
    // errors — it must never answer for a different agent. A trailing
    // (or sole) integer is a line count (`/read w8:p1 50`, `/read 50`),
    // like topics; pane ids/kinds are never bare numbers, so a numeric
    // word that resolves as a target stays a target. Wide parse (i64)
    // so huge counts clamp to 400 instead of erroring on u32 overflow.
    let (target_arg, lines) = match arg.rsplit_once(char::is_whitespace) {
        Some((head, tail)) => match tail.parse::<i64>() {
            Ok(n) if resolve_target(rows, Some(arg)).is_none() => {
                let head = head.trim();
                let count = n.clamp(1, READ_CAP as i64) as u32;
                (if head.is_empty() { None } else { Some(head) }, count)
            }
            _ => (Some(arg), TOPIC_READ_DEFAULT),
        },
        None => match arg.parse::<i64>() {
            Ok(n) if resolve_target(rows, Some(arg)).is_none() => {
                (None, n.clamp(1, READ_CAP as i64) as u32)
            }
            _ => (if arg.is_empty() { None } else { Some(arg) }, TOPIC_READ_DEFAULT),
        },
    };
    // Corpse reply with exactly one live agent: the sole-agent shortcut
    // below would otherwise serve (and refocus) the wrong session.
    // Explicit targets keep their own unknown-target error.
    if target_arg.is_none() && unmatched_reply(rows, reply_pane) {
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
            .await;
        return;
    }
    let mut row = match resolve_target(rows, target_arg) {
        Some(r) => Some(r),
        None if target_arg.is_some() => {
            s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                .await;
            return;
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
    let Some(row) = row else {
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
            .await;
        return;
    };
    match read_agent_output(&s.cfg.socket, &row.pane, lines).await {
        Ok(out) => {
            let body = if out.is_empty() {
                crate::ui::NO_OUTPUT.into()
            } else {
                out
            };
            let mid = s.tg.send_msg(chat, None, &body, None).await;
            s.remember(chat, mid, &row.pane).await;
            s.set_focus(&row.pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None).await;
        }
    }
}

pub(crate) async fn handle_status(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    // Corpse reply with exactly one live agent: the sole-agent shortcut
    // would otherwise serve (and refocus) the wrong session. Explicit
    // targets win over the reply, so only bare replies refuse here.
    if arg.is_empty() && unmatched_reply(rows, reply_pane) {
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
            .await;
        return;
    }
    let mut pane = match resolve_target(rows, if arg.is_empty() { None } else { Some(arg) }) {
        Some(r) => Some(r.pane),
        // Explicit but unknown: never show a different agent's card.
        None if !arg.is_empty() => {
            s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                .await;
            return;
        }
        None => reply_pane.clone(),
    };
    if pane.is_none() {
        if let Some(f) = s
            .get_focus()
            .await
            .filter(|f| rows.iter().any(|r| &r.pane == f))
        {
            pane = Some(f);
        } else {
            pane = resolve_target(rows, Some("")).map(|r| r.pane);
        }
    }
    let Some(pane) = pane else {
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
            .await;
        return;
    };
    match get_agent(&s.cfg.socket, &pane).await {
        Ok(agent) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let space = ws_label(&spaces, &agent.ws);
            let mid =
                s.tg.send_msg(
                    chat,
                    None,
                    &build_agent_card_text(&agent, space),
                    Some(agent_card_kb(&pane, &agent.ws, space)),
                )
                .await;
            s.remember(chat, mid, &pane).await;
            s.set_focus(&pane).await;
        }
        Err(e) => {
            s.tg.send_msg(
                chat,
                None,
                &format!("⚠️ status failed: {} — try /agents", crate::types::mask_home(&e.to_string())),
                None,
            )
            .await;
        }
    }
}
