use crate::{
    herdr::client::{create_workspace, get_agent, list_agents, list_workspaces, spawn_agent},
    state::AppState,
    ui::{agent_card_kb, build_agent_card_text, build_menu_text, main_menu_kb, ws_label},
};

pub(crate) async fn handle_new_space(s: &AppState, chat: i64, msg_id: i64, thread: Option<i64>) {
    s.tg.edit_msg(chat, msg_id, "⏳ creating space…", None)
        .await;
    let label = super::space::next_label(s).await;
    let ws_id = match create_workspace(&s.cfg.socket, &label).await {
        Ok(id) => id,
        Err(e) => {
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("⚠️ failed to create space: {e}"),
                None,
            )
            .await;
            return;
        }
    };
    super::shell::open_shell(s, chat, thread, Some(&ws_id)).await;
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
    s.tg.edit_msg(
        chat,
        msg_id,
        &build_menu_text(&spaces, &agents),
        Some(main_menu_kb(&spaces, &agents)),
    )
    .await;
}

pub(crate) async fn handle_spawn(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    kind: &str,
    ws: Option<&str>,
) {
    let label = ws.unwrap_or("tg");
    s.tg.edit_msg(
        chat,
        msg_id,
        &format!("⏳ starting {kind} in {label} space…"),
        None,
    )
    .await;
    match spawn_agent(&s.cfg.socket, kind, ws).await {
        Ok(row) => {
            s.remember(chat, Some(msg_id), &row.pane).await;
            s.set_focus(&row.pane).await;
            if let Ok(agent) = get_agent(&s.cfg.socket, &row.pane).await {
                // Ensure topic exists in forum group if enabled
                if s.cfg.forum.is_some() {
                    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                    let sp = ws_label(&spaces, &agent.ws);
                    s.topics.sync_topic(&agent.pane, &agent.kind, sp).await;
                }
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &build_agent_card_text(&agent),
                    Some(agent_card_kb(&row.pane, &agent.ws)),
                )
                .await;
            }
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ spawn failed: {e}"), None)
                .await;
        }
    }
}
