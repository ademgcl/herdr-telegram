use super::callback_parse::{gone_card, live_target, split_action, split_head};
use super::callback_waiters::{
    handle_agent_output, handle_keys_arm, handle_pane_output, handle_run_arm,
};
use crate::{
    herdr::client::{get_agent, list_agents, list_panes, list_workspaces},
    state::{
        AppState, OpGuard,
        guard::{SPAWNDEDUP_SECS, SPAWNOP_STALE_SECS},
    },
    ui::{
        agent_card_kb, build_agent_card_text, build_menu_text, build_ws_text, main_menu_kb,
        spawn_kb, workspace_kb, ws_label,
    },
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn handle_callback(s: AppState, cbq: &Value) {
    // Answer the spinner FIRST, even for rejected taps (split to
    // `callback_stale`: spawned, never awaited — see there).
    super::callback_stale::answer_spinner(&s, cbq);
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

    let (Some(chat), Some(msg_id)) = (chat, msg_id) else {
        return;
    };

    // Thread for ack messages (forum topics carry it, DMs don't).
    let thread = cbq["message"]["message_thread_id"].as_i64();
    // Stale cards must not re-execute (days-old spawn-confirm/kill taps):
    // one gate (STALE_SECS) covers both relic and merely outdated taps.
    // Dialog (B) taps are exempt — they re-validate against the live
    // pane at tap time (blocked-status gate + shape checks), so a
    // long-lived blocked card stays tappable while destructive arms keep
    // the birth-date gate. Model taps are gated except the read-only list
    // re-render (M:list:<pane>); an M:<idx> switch is a side effect.
    let (head, rest0) = split_head(data);
    if !super::callback_stale::tap_exempt(head, rest0) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Fail-open: staleness must be proven — a callback without
        // `message.date` (malformed/channel shape) defaults to now so a
        // legitimate tap is never dropped as a day-0 relic (0 is always
        // stale under `tap_stale`).
        let date = cbq["message"]["date"].as_u64().unwrap_or(now);
        if super::callback_stale::tap_stale(date, now) {
            super::callback_stale::notify_stale_card(&s, chat, thread);
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
        ("N", None) => {
            // Single-flight: a double-tap would mint two spaces + agents
            // (real billable resources). Own `spawnop` map — synthetic
            // keys are never live panes, so pane-liveness hygiene must
            // never touch them (age-only expiry in hygiene). Persistent
            // `spawndone` dedup covers SEQUENTIAL double-taps too (the
            // pump awaits each update: a transient guard drops between
            // queued taps). Stamped on success only — a pre-mint failure
            // keeps the same card retappable (the error edit explains).
            // N and k mint different resources from the same card (a new
            // space edits the card to a menu that k taps follow): one
            // shared `spawn:` key would refuse the legitimate k after an
            // N, so keys carry the action.
            let key = format!("spawn:N:{chat}:{msg_id}");
            {
                let done = s.spawndone.lock().await;
                if let Some(at) = done.get(&key)
                    && !crate::state::guard::claim_stale(
                        *at,
                        std::time::Instant::now(),
                        SPAWNDEDUP_SECS,
                    )
                {
                    return;
                }
            }
            let _guard = match OpGuard::claim_limited(&s.spawnop, &key, SPAWNOP_STALE_SECS).await {
                Some(g) => g,
                None => return,
            };
            if super::callback_spawn::handle_new_space(&s, chat, msg_id, thread).await {
                super::callback_spawn::stamp_spawndone(&s, key).await;
            }
        }
        ("k", Some(r)) => {
            // Action-scoped like N above, plus the target: a replay of
            // the same tap still stands down while a different kind stays
            // tappable.
            let key = format!("spawn:k:{r}:{chat}:{msg_id}");
            {
                let done = s.spawndone.lock().await;
                if let Some(at) = done.get(&key)
                    && !crate::state::guard::claim_stale(
                        *at,
                        std::time::Instant::now(),
                        SPAWNDEDUP_SECS,
                    )
                {
                    return;
                }
            }
            let _guard = match OpGuard::claim_limited(&s.spawnop, &key, SPAWNOP_STALE_SECS).await {
                Some(g) => g,
                None => return,
            };
            let minted = match r.split_once(':') {
                Some((ws, kind)) => {
                    super::callback_spawn::handle_spawn(&s, chat, msg_id, kind, Some(ws)).await
                }
                None => super::callback_spawn::handle_spawn(&s, chat, msg_id, r, None).await,
            };
            if minted {
                super::callback_spawn::stamp_spawndone(&s, key).await;
            }
        }
        ("K", Some(pane)) => {
            handle_keys_arm(&s, chat, msg_id, thread, pane).await;
        }
        ("R", Some(ws)) => {
            handle_run_arm(&s, chat, msg_id, thread, ws).await;
        }
        ("p", Some(pane)) => {
            handle_pane_output(&s, chat, msg_id, thread, pane).await;
        }
        ("m", None) => {
            let (spaces, agents) = match (
                list_workspaces(&s.cfg.socket).await,
                list_agents(&s.cfg.socket).await,
            ) {
                (Ok(spaces), Ok(agents)) => (spaces, agents),
                _ => {
                    s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                }
            };
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
            let (spaces, agents) = match (
                list_workspaces(&s.cfg.socket).await,
                list_agents(&s.cfg.socket).await,
            ) {
                (Ok(spaces), Ok(agents)) => (spaces, agents),
                _ => {
                    s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                }
            };
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
                    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                    let space = ws_label(&spaces, &agent.ws);
                    s.tg.edit_msg(
                        chat,
                        msg_id,
                        &build_agent_card_text(&agent, space),
                        Some(agent_card_kb(pane, &agent.ws, space)),
                    )
                    .await;
                }
                Err(_) => {
                    // Fail-closed like the K/p/o arms: an unreadable herdr
                    // never moves focus, remembers routing, or edits a
                    // shell card (ambiguous read → no write, visible retry).
                    match list_panes(&s.cfg.socket).await {
                        Ok(l) if l.contains(&pane.to_string()) => {
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
                        }
                        Ok(_) => {
                            gone_card(&s, chat, msg_id, pane).await;
                        }
                        Err(_) => {
                            s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                                .await;
                        }
                    }
                }
            }
        }
        ("o", Some(pane)) => {
            handle_agent_output(&s, chat, msg_id, thread, pane).await;
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
                s.tg.edit_msg(chat, msg_id, crate::ui::UNKNOWN_BUTTON, None)
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
                s.tg.edit_msg(chat, msg_id, crate::ui::UNKNOWN_BUTTON, None)
                    .await;
            }
        }
        _ => {
            s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_BUTTON, None)
                .await;
        }
    }
}
