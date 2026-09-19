use super::shell_provision::attach_shell_pane;
use crate::{
    herdr::client::{best_split_direction, list_workspaces, pane_layout, split_pane},
    herdr::labels::pane_facts,
    state::AppState,
    ui::ws_label,
};

/// Split the pane sideways in the SAME tab: sibling shell pane + its own
/// topic, named/provisioned like any shell. Empty `dir` picks the pane's
/// longer axis from live layout (explicit right|down always wins);
/// unreadable layout falls back to right.
pub async fn open_split(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, dir: &str) {
    let dir = if dir.is_empty() {
        auto_split_dir(&s.cfg.socket, pane).await
    } else {
        dir.to_string()
    };
    let new = match split_pane(&s.cfg.socket, pane, &dir).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(
                chat,
                thread,
                &format!(
                    "⚠️ split failed: {}",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let ws_id = pane_facts(&s.cfg.socket)
        .await
        .ok()
        .and_then(|m| m.get(pane).map(|f| f.ws.clone()))
        .unwrap_or_default();
    let space = ws_label(&spaces, &ws_id).to_string();
    let space = if space.is_empty() { pane } else { &space };
    attach_shell_pane(s, chat, thread, &new, space, false).await;
}

/// Bare-`/split` direction from live tab geometry: the target pane's
/// longer axis wins. Any unreadable step falls back to right (the old
/// bare default) — a failed probe must never block the split.
async fn auto_split_dir(socket: &str, pane: &str) -> String {
    let tab = pane_facts(socket)
        .await
        .ok()
        .and_then(|m| m.get(pane).map(|f| f.tab_id.clone()))
        .filter(|t| !t.is_empty());
    let Some(tab) = tab else {
        return "right".to_string();
    };
    match pane_layout(socket, &tab)
        .await
        .ok()
        .and_then(|m| m.get(pane).copied())
    {
        Some((w, h)) => best_split_direction(w, h).to_string(),
        None => "right".to_string(),
    }
}
