//! Agent-card tap (`a:<pane>`): split from `callback` (300-line file limit).
use super::callback_parse::gone_card;
use crate::{
    herdr::client::{get_agent, list_panes, list_workspaces},
    state::AppState,
    ui::{agent_card_kb, build_agent_card_text, ws_label},
};

/// Re-render the agent card in place. Routing follows delivery (M:list
/// parity): a deleted card pins neither focus nor reply-routing.
pub(crate) async fn handle_agent_card(s: &AppState, chat: i64, msg_id: i64, pane: &str) {
    match get_agent(&s.cfg.socket, pane).await {
        Ok(agent) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let space = ws_label(&spaces, &agent.ws);
            if s
                .tg
                .try_edit_msg(
                    chat,
                    msg_id,
                    &build_agent_card_text(&agent, space),
                    Some(agent_card_kb(pane, &agent.ws, space)),
                )
                .await
                .is_ok()
            {
                s.remember(chat, Some(msg_id), pane).await;
                s.set_focus(pane).await;
            }
        }
        Err(_) => {
            // Fail-closed like the K/p/o arms: an unreadable herdr
            // never moves focus, remembers routing, or edits a
            // shell card (ambiguous read → no write, visible retry).
            match list_panes(&s.cfg.socket).await {
                Ok(l) if l.contains(&pane.to_string()) => {
                    // Agent gone but the shell lives: deliberate taps
                    // may still navigate here — focus moves, but there
                    // is no agent card to show. Remember stays
                    // delivery-gated: a deleted card must not pin
                    // reply-routing to a dead msg_id.
                    s.set_focus(pane).await;
                    if s
                        .tg
                        .try_edit_msg(
                            chat,
                            msg_id,
                            &crate::ui::shell_gone_text(pane),
                            None,
                        )
                        .await
                        .is_ok()
                    {
                        s.remember(chat, Some(msg_id), pane).await;
                    }
                }
                Ok(_) => {
                    gone_card(s, chat, msg_id, pane).await;
                }
                Err(_) => {
                    s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                }
            }
        }
    }
}
