//! Drop a pane's agent to a shell: idle/done quits directly, busy
//! agents confirm first (stateless buttons, kill pattern). Success
//! badges shell immediately (kind + bot-owned icon), so the topic
//! reflects the flip without waiting for the watchdog.
use super::shell_common::shell_card_text;
use crate::{
    herdr::client::{get_agent, list_agents, list_panes, send_agent_keys},
    state::AppState,
    topics::names,
};
use serde_json::json;
use tokio::time::{Duration, sleep};

/// A missing agent row really is a shell (not a list dropout) when the
/// volatile status or the durable tag already says shell — shared with
/// the watchdog skip so flips converge instead of skipping forever.
pub async fn confirmed_shell(s: &AppState, pane: &str) -> bool {
    if s.status
        .lock()
        .await
        .get(pane)
        .is_some_and(|st| st == "shell")
    {
        return true;
    }
    s.topics.is_shell_tagged(pane)
}

/// Pure confirm copy so tests cover it without I/O.
pub fn quit_confirm_text(pane: &str, status: &str) -> String {
    format!(
        "⚠️ quit {pane} while it is {status}?\nThis kills live work — the agent will not finish."
    )
}

/// Drop the pane's agent to a shell. Idle/done quits directly; busy
/// agents confirm first — the tap carries the kill order.
pub async fn quit_to_shell(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let agent = match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => a,
        Err(_) => {
            // Dead pane (topic lingering) vs live shell — only the latter
            // gets the shell card. Fail-closed throughout: a row that
            // still lists the pane means a blip (one retry, then refuse),
            // an unreadable list refuses — never a shell card over a
            // live agent.
            let rows = match list_agents(&s.cfg.socket).await {
                Err(_) => {
                    s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                }
                Ok(rows) => rows,
            };
            if rows.iter().any(|r| r.pane == pane) {
                match get_agent(&s.cfg.socket, pane).await {
                    Ok(a) => a,
                    Err(_) => {
                        s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                            .await;
                        return;
                    }
                }
            } else {
                // `list_agents` can drop a row transiently: re-probe the
                // agent once before claiming a shell, or a blip mints a
                // false shell card + steals focus over a live agent.
                let Ok(a) = get_agent(&s.cfg.socket, pane).await else {
                    let dead = match list_panes(&s.cfg.socket).await {
                        Ok(l) => !l.contains(&pane.to_string()),
                        Err(_) => {
                            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                                .await;
                            return;
                        }
                    };
                    if dead {
                        s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_TARGET, None)
                            .await;
                        return;
                    }
                    let mid =
                        s.tg.send_msg(chat, thread, &shell_card_text(pane), None)
                            .await;
                    s.remember(chat, mid, pane).await;
                    if mid.is_some() {
                        s.set_focus(pane).await; // delivery-gated (provision parity)
                    }
                    return;
                };
                a
            }
        }
    };
    if !matches!(agent.status.as_str(), "idle" | "done") {
        // Stateless confirm (no pending-state map to leak): Quit/Keep taps carry the pane, like X:kill.
        let kb = json!([[
            {"text": "Quit anyway", "callback_data": format!("X:quit:{pane}")},
            {"text": "Keep", "callback_data": format!("X:keep:{pane}")},
        ]]);
        let mid =
            s.tg.send_msg(
                chat,
                thread,
                &quit_confirm_text(pane, &agent.status),
                Some(kb),
            )
            .await;
        s.remember(chat, mid, pane).await;
        return;
    }
    do_quit(s, chat, thread, pane, None, false).await;
}

/// Confirmed quit tap (`X:quit`; `X:keep` is shared with kill cards
/// and handled there, never here).
pub async fn handle_quit_action(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    action: &str,
    pane: &str,
) {
    if action != "quit" {
        // Fail-closed unknown-action guard (mirrors kill); `keep`
        // routes to the kill handler, never here.
        return;
    }
    // Confirmed kill order: a live watcher dies quietly first (its
    // cards would fight the quit ack); a failed quit then leaves
    // disarmed waiters — accepted, the work was ordered dead.
    s.cancel_jobs_for_quiet(pane).await;
    do_quit(s, chat, thread, pane, Some(msg_id), true).await;
}

/// Error delivery: fresh message for /quit, in-place edit for taps.
async fn quit_say(s: &AppState, chat: i64, thread: Option<i64>, edit_mid: Option<i64>, msg: &str) {
    match edit_mid {
        Some(mid) => {
            s.tg.edit_msg(chat, mid, msg, None).await;
        }
        None => {
            s.tg.send_msg(chat, thread, msg, None).await;
        }
    }
}

/// Shared interrupt flow: ctrl+c, then poll for the shell. Confirmed
/// busy agents may land idle first (run interrupted, process alive)
/// and need another ctrl+c to drop (REPL behavior) — hence the longer
/// budget plus re-keys. Unconfirmed keeps the old 4×1.5s single-keys.
async fn do_quit(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    edit_mid: Option<i64>,
    confirmed: bool,
) {
    // Waiters are NOT cleared here: a failed quit (keys, still-busy)
    // must leave them armed for the retry. Success clears via
    // cancel_jobs_for → clear_waiters below.
    if send_agent_keys(&s.cfg.socket, pane, &["ctrl+c"])
        .await
        .is_err()
    {
        quit_say(s, chat, thread, edit_mid, crate::ui::QUIT_KEYS_FAILED_PC).await;
        return;
    }
    // Confirm herdr sees a shell (agent_not_found) before claiming it.
    let mut keys = 1u32;
    let mut seen_kind: Option<String> = None;
    let mut shelled = false;
    for _ in 0..if confirmed { 8 } else { 4 } {
        sleep(Duration::from_millis(1500)).await;
        match get_agent(&s.cfg.socket, pane).await {
            // Confirmed death only: any other error is a blip (timeout,
            // dropout) that must keep polling, never claim the shell — a
            // false claim wipes jobs/intent over a live agent (fail-open).
            Err(e) if crate::herdr::rpc::is_not_found(&e.to_string()) => {
                shelled = true;
                break;
            }
            Err(_) => {}
            Ok(a) => {
                seen_kind = Some(a.kind.clone());
                if confirmed
                    && keys < 3
                    && matches!(a.status.as_str(), "idle" | "done")
                    && send_agent_keys(&s.cfg.socket, pane, &["ctrl+c"])
                        .await
                        .is_ok()
                {
                    keys += 1;
                }
            }
        }
    }
    if !shelled {
        quit_say(
            s,
            chat,
            thread,
            edit_mid,
            &format!(
                "⚠️ still in {} — try again or quit on the PC",
                seen_kind.as_deref().unwrap_or("agent")
            ),
        )
        .await;
        return;
    }
    s.cancel_jobs_for(pane).await;
    // Strip dead ⛔ buttons first (clear_pane drops tracking, shared helper).
    crate::handlers::dialog::resolve_cards_unless_held(s, pane).await;
    s.clear_pane(pane).await;
    // Badge shell NOW (not on the next watchdog cycle) so a fast
    // re-enter still hushes correctly in the notifier.
    s.status
        .lock()
        .await
        .insert(pane.to_string(), "shell".to_string());
    // Uniform prune-retire (clear_pane inlines the sig/card clears, so
    // this only covers a post-clear raced prune — a no-op otherwise).
    if s.topics.mark_shell(pane).await {
        crate::handlers::dialog::retire_dialog(s, pane).await;
    }
    // Record the flip NOW (watchdog skips unknown-missing rows).
    s.topics.note_kind(pane, "shell");
    // Immediate bot-owned re-icon (user customs kept); failures retry
    // next tick via the watchdog flip path above.
    if let (Some(forum), Some(topic)) = (s.cfg.forum, s.topics.all_mappings().get(pane).copied())
        && let Some(want) =
            names::icon_needs_update(s.topics.storage.get_icon(pane).as_deref(), "shell")
        && s.tg.set_topic_icon(forum, topic, want).await.is_ok()
    {
        s.topics.storage.set_icon(pane, want);
    }
    match edit_mid {
        Some(mid) => {
            s.tg.edit_msg(chat, mid, &shell_card_text(pane), None).await;
            s.remember(chat, Some(mid), pane).await;
        }
        None => {
            let mid =
                s.tg.send_msg(chat, thread, &shell_card_text(pane), None)
                    .await;
            s.remember(chat, mid, pane).await;
        }
    }
    s.set_focus(pane).await;
}

/// Shell→agent flip watcher: after shell input the pane may have become
/// an agent. Polls briefly and badges immediately (mirror of `do_quit`
/// above). Spawned, never awaited; reads plus a conditional bot-owned
/// icon write (customs kept, `?`/errors write nothing).
pub(crate) fn spawn_flip_watch(s: &AppState, pane: &str) {
    let s = s.clone();
    let pane = pane.to_string();
    tokio::spawn(async move {
        for _ in 0..8 {
            sleep(Duration::from_millis(1500)).await;
            match get_agent(&s.cfg.socket, &pane).await {
                Ok(a) if a.kind != "?" && a.kind != "shell" => {
                    s.status.lock().await.insert(pane.clone(), a.status.clone());
                    s.topics.note_kind(&pane, &a.kind);
                    if let (Some(forum), Some(thread)) =
                        (s.cfg.forum, s.topics.all_mappings().get(&pane).copied())
                        && let Some(want) = names::icon_needs_update(
                            s.topics.storage.get_icon(&pane).as_deref(),
                            &a.kind,
                        )
                        && s.tg.set_topic_icon(forum, thread, want).await.is_ok()
                    {
                        s.topics.storage.set_icon(&pane, want);
                    }
                    break;
                }
                // Still shell (`Err`) or unknown kind (`?`): keep polling —
                // agent registration lags shell input by seconds.
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quit_confirm_text() {
        let t = quit_confirm_text("w1:p1", "working");
        assert!(t.contains("w1:p1"));
        assert!(t.contains("working"));
        assert!(t.contains("kills live work"));
    }
}
