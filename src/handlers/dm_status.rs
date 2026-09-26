use super::target::{resolve_target, unmatched_reply};
use crate::{
    herdr::client::{get_agent, list_panes, list_workspaces},
    state::AppState,
    types::AgentRow,
    ui::{agent_card_kb, build_agent_card_text, ws_label},
};

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
        // Rowless focus must not shadow to sole-agent (cross-pane card).
        match s.get_focus().await {
            Some(f) if rows.iter().any(|r| r.pane == f) => {
                pane = Some(f);
            }
            Some(_) => {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
                return;
            }
            None => {
                pane = resolve_target(rows, Some("")).map(|r| r.pane);
            }
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
            // Focus follows delivery: a failed card pins no routing.
            if mid.is_some() {
                s.set_focus(&pane).await;
            }
        }
        Err(_) => {
            // Gone probe (callback `a:`-arm parity): a corpse pane must
            // report UNKNOWN_TARGET, a live shell its shell notice — a
            // bare "status failed" for both hides death as error. An
            // unreadable herdr refuses visibly, never guesses.
            match list_panes(&s.cfg.socket).await {
                Ok(l) if l.contains(&pane) => {
                    let mid =
                        s.tg.send_msg(chat, None, &crate::ui::shell_gone_text(&pane), None)
                            .await;
                    s.remember(chat, mid, &pane).await;
                    // Focus follows delivery (card arm parity above).
                    if mid.is_some() {
                        s.set_focus(&pane).await;
                    }
                }
                Ok(_) => {
                    s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                        .await;
                }
                Err(_) => {
                    s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                }
            }
        }
    }
}
