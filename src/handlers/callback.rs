use super::callback_parse::{gone_card, live_target, pane_live, split_action, split_head};
use crate::{
    herdr::client::{get_agent, list_agents, list_workspaces, read_agent_output, read_pane_output},
    state::AppState,
    ui::{
        agent_card_kb, btn, build_agent_card_text, build_menu_text, build_ws_text, main_menu_kb,
        pane_output_kb, spawn_kb, workspace_kb,
    },
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn handle_callback(s: AppState, cbq: &Value) {
    let Some(from) = cbq["from"]["id"].as_i64() else {
        return;
    };
    if !s.cfg.owners.contains(&from) {
        // Intrusion attempts log the event, never the sender id.
        println!("[callback] ignoring non-owner tap");
        return;
    }
    let chat = cbq["message"]["chat"]["id"].as_i64();
    let msg_id = cbq["message"]["message_id"].as_i64();
    let data = cbq["data"].as_str().unwrap_or("");

    if let Some(cbq_id) = cbq["id"].as_str() {
        s.tg.answer_callback(cbq_id).await;
    }
    let (Some(chat), Some(msg_id)) = (chat, msg_id) else {
        return;
    };

    // Thread for ack messages (forum topics carry it, DMs don't).
    let thread = cbq["message"]["message_thread_id"].as_i64();
    // Stale cards must not re-execute (days-old spawn-confirm/kill taps):
    // one gate covers both relic (86400s+) and merely outdated taps.
    // Dialog (B) taps are exempt — they re-validate against the live
    // pane at tap time (count/shape/footer checks), so a long-lived
    // blocked card stays tappable while destructive arms keep the
    // birth-date gate. Model taps are gated except the read-only list
    // re-render (M:list:<pane>); an M:<idx> switch is a side effect.
    let (head, rest0) = split_head(data);
    let exempt =
        head == "B" || (head == "M" && rest0.map(|r| r.starts_with("list:")).unwrap_or(false));
    if !exempt {
        let date = cbq["message"]["date"].as_u64().unwrap_or(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(date) > crate::types::STALE_SECS {
            println!("[callback] dropping stale tap");
            s.tg.send_msg(
                chat,
                thread,
                "⌛️ that card expired — /card for a fresh one",
                None,
            )
            .await;
            return;
        }
    }
    let (head, rest) = split_head(data);
    match (head, rest) {
        ("n", None) => {
            s.tg.edit_msg(chat, msg_id, "spawn which agent?", Some(spawn_kb(None)))
                .await;
            s.forget_target(chat, msg_id).await;
        }
        ("n", Some(ws)) => {
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("spawn into {ws}: which agent?"),
                Some(spawn_kb(Some(ws))),
            )
            .await;
            s.forget_target(chat, msg_id).await;
        }
        ("N", None) => super::callback_spawn::handle_new_space(&s, chat, msg_id, thread).await,
        ("k", Some(r)) => match r.split_once(':') {
            Some((ws, kind)) => {
                super::callback_spawn::handle_spawn(&s, chat, msg_id, kind, Some(ws)).await
            }
            None => super::callback_spawn::handle_spawn(&s, chat, msg_id, r, None).await,
        },
        ("K", Some(pane)) => {
            if !pane_live(&s, pane).await {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            }
            // Exclusive waiter: a sibling run/type waiter for this key
            // would otherwise win the next message instead.
            s.runwait.lock().await.remove(&(chat, thread));
            s.typewait.lock().await.remove(&(chat, thread));
            s.keywait
                .lock()
                .await
                .insert((chat, thread), pane.to_string());
            s.set_focus(pane).await;
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"),
                None,
            )
            .await;
        }
        ("R", Some(ws)) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            if !spaces.iter().any(|w| w.id == *ws) {
                s.tg.edit_msg(chat, msg_id, &format!("workspace {ws} is gone"), None)
                    .await;
                s.forget_target(chat, msg_id).await;
                return;
            }
            // Exclusive waiter (see K arm).
            s.keywait.lock().await.remove(&(chat, thread));
            s.typewait.lock().await.remove(&(chat, thread));
            s.runwait
                .lock()
                .await
                .insert((chat, thread), ws.to_string());
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("⌨️ send shell command for {ws}\nnext message = command"),
                None,
            )
            .await;
        }
        ("p", Some(pane)) => {
            let Ok(out) = read_pane_output(&s.cfg.socket, pane, 120).await else {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            };
            let body = if out.is_empty() {
                "(no output)".into()
            } else {
                out
            };
            let mid =
                s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(pane)))
                    .await;
            s.remember(chat, mid, pane).await;
        }
        ("m", None) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            s.tg.edit_msg(
                chat,
                msg_id,
                &build_menu_text(&spaces, &agents),
                Some(main_menu_kb(&spaces, &agents)),
            )
            .await;
            s.forget_target(chat, msg_id).await;
        }
        ("w", Some(ws)) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            let my_agents: Vec<_> = agents.into_iter().filter(|a| a.ws == *ws).collect();
            s.tg.edit_msg(
                chat,
                msg_id,
                &build_ws_text(ws, &spaces, &my_agents),
                Some(workspace_kb(ws, &my_agents)),
            )
            .await;
            s.forget_target(chat, msg_id).await;
        }
        ("a", Some(pane)) => {
            match get_agent(&s.cfg.socket, pane).await {
                Ok(agent) => {
                    s.remember(chat, Some(msg_id), pane).await;
                    s.set_focus(pane).await;
                    s.tg.edit_msg(
                        chat,
                        msg_id,
                        &build_agent_card_text(&agent),
                        Some(agent_card_kb(pane, &agent.ws)),
                    )
                    .await;
                }
                Err(_) => {
                    if pane_live(&s, pane).await {
                        // Agent gone but the shell lives: deliberate taps
                        // may still navigate here — focus moves, but there
                        // is no agent card to show.
                        s.remember(chat, Some(msg_id), pane).await;
                        s.set_focus(pane).await;
                        s.tg.edit_msg(
                            chat,
                            msg_id,
                            &format!("{pane} is now a shell pane (agent gone)"),
                            None,
                        )
                        .await;
                    } else {
                        gone_card(&s, chat, msg_id, pane).await;
                    }
                }
            }
        }
        ("o", Some(pane)) => {
            let Ok(out) = read_agent_output(&s.cfg.socket, pane, 120).await else {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            };
            let body = if out.is_empty() {
                "(no output)".into()
            } else {
                out
            };
            let kb = serde_json::json!([[btn("← back", &format!("a:{pane}"))]]);
            let mid = s.tg.send_msg(chat, thread, &body, Some(kb)).await;
            s.remember(chat, mid, pane).await;
            s.set_focus(pane).await;
        }
        // Blocked-pane answers: B:<action>:<pane> — the tapped card is
        // updated in place (a turned-over dialog swaps question+buttons).
        ("B", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            if let Some((action, pane)) = split_action(r) {
                super::tap::answer_tap(&s, chat, msg_id, thread, pane, action).await;
            } else {
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    "unknown button — tap again from a fresh card",
                    None,
                )
                .await;
            }
        }
        // Model picker: M:<idx>:<pane> free-Zen taps, M:list:<pane> card.
        ("M", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            super::callback_model::handle_model_tap(&s, chat, msg_id, thread, r).await
        }
        // Pane kill/quit confirm: X:<kill|quit|keep>:<pane> — stateless buttons.
        ("X", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            if let Some((action, pane)) = split_action(r) {
                // Keep is shared: both cards only need the kept ack.
                if action == "quit" {
                    super::shell::handle_quit_action(&s, chat, msg_id, thread, action, pane).await;
                } else {
                    super::kill::handle_kill_action(&s, chat, msg_id, action, pane).await;
                }
            } else {
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    "unknown button — tap again from a fresh card",
                    None,
                )
                .await;
            }
        }
        _ => {
            s.tg.send_msg(chat, thread, "unknown button — /card for a fresh one", None)
                .await;
        }
    }
}
