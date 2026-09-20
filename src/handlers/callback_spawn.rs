use crate::{
    herdr::client::{create_workspace, get_agent, list_agents, list_workspaces, spawn_agent},
    state::AppState,
    ui::{agent_card_kb, build_agent_card_text, build_menu_text, main_menu_kb, ws_label},
};
use std::{collections::HashMap, time::Instant};

/// Disk copy of the spawn dedup (`spawn:<action>:…:<chat>:<msg>` per
/// line): the offset acks AFTER handling, so a crash between mint and ack replays
/// the tap inside STALE_SECS — RAM-only dedup would double-mint
/// billable resources. Loaded at boot with every entry counting as
/// fresh (over-dedup fails safe: the owner retaps a fresh card with a
/// fresh key). Rewritten on every stamp from live RAM keys, so stale
/// disk entries vanish after the first post-boot spawn (no hygiene).
fn spawn_done_path() -> std::path::PathBuf {
    crate::state::state_dir().join("spawn.done")
}

/// Boot load for `State.spawndone` (see module docs on freshness).
pub(crate) fn load_spawndone() -> HashMap<String, Instant> {
    let now = Instant::now();
    std::fs::read_to_string(spawn_done_path())
        .map(|txt| {
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| (l.to_string(), now))
                .collect()
        })
        .unwrap_or_default()
}

/// Stamp one minted key (RAM + disk, single lock, disk after).
pub(crate) async fn stamp_spawndone(s: &AppState, key: String) {
    let keys: Vec<String> = {
        let mut done = s.spawndone.lock().await;
        done.insert(key, Instant::now());
        done.keys().cloned().collect()
    };
    // Same dedup set the next boot loads: stale lines never survive a
    // post-boot stamp (only live RAM keys persist).
    let body = keys.join("\n");
    let _ = crate::types::write_private(&spawn_done_path(), body.as_bytes());
}

/// True when a billable resource was minted (workspace / agent): the
/// caller stamps the persistent spawn dedup so a sequential retap stands
/// down. False (pre-mint failure) leaves the key unstamped — the same
/// card stays retappable, and the error edit already explains why.
pub(crate) async fn handle_new_space(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
) -> bool {
    s.tg.edit_msg(chat, msg_id, "⏳ creating space…", None)
        .await;
    // Auto-label needs a readable list: an outage refuses before any
    // billable mint, never guesses a colliding `space-1`.
    let Some(label) = super::space::next_label(s).await else {
        s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
            .await;
        return false;
    };
    // create_workspace fail-closed (empty→Err): no dead Ok-empty arm.
    let ws_id = match create_workspace(&s.cfg.socket, &label).await {
        Ok(id) => id,
        Err(e) => {
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!(
                    "⚠️ failed to create space: {}",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
            return false;
        }
    };
    super::shell::open_space_shell(s, chat, thread, &ws_id).await;
    // Post-mint render is fail-closed like the pre-mint paths: a herdr
    // outage after the billable mint must not overwrite the card with an
    // empty menu (the space exists — the owner retaps a fresh card).
    let (spaces, agents) = match (
        list_workspaces(&s.cfg.socket).await,
        list_agents(&s.cfg.socket).await,
    ) {
        (Ok(spaces), Ok(agents)) => (spaces, agents),
        _ => {
            s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return true;
        }
    };
    s.tg.edit_msg(
        chat,
        msg_id,
        &build_menu_text(&spaces, &agents),
        Some(main_menu_kb(&spaces, &agents)),
    )
    .await;
    true
}

pub(crate) async fn handle_spawn(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    kind: &str,
    ws: Option<&str>,
) -> bool {
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
                let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                let space = ws_label(&spaces, &agent.ws);
                // Ensure topic exists in forum group if enabled
                if s.cfg.forum.is_some() {
                    // Fresh pane (see agents.rs): retire only on a raced prune.
                    if s.topics
                        .sync_topic_prune(&agent.pane, &agent.kind, space)
                        .await
                        .1
                    {
                        crate::handlers::dialog::retire_dialog(s, &agent.pane).await;
                    }
                }
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &build_agent_card_text(&agent, space),
                    Some(agent_card_kb(&row.pane, &agent.ws, space)),
                )
                .await;
            } else {
                // Spawned but status unreadable (herdr blip): still land
                // the topic + ack instead of hanging on "starting…".
                if s.cfg.forum.is_some() {
                    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                    let sp = ws_label(&spaces, &row.ws);
                    if s.topics.sync_topic_prune(&row.pane, &row.kind, sp).await.1 {
                        crate::handlers::dialog::retire_dialog(s, &row.pane).await;
                    }
                }
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &format!(
                        "✅ Started {} [{}] (status unreadable — try /status)",
                        row.kind, row.pane
                    ),
                    None,
                )
                .await;
            }
            // Minted: the agent exists even when the status read blipped.
            true
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &crate::ui::spawn_failed(&e.to_string()), None)
                .await;
            false
        }
    }
}
