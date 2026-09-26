use super::target::resolve_target;
use crate::{herdr::client::list_agents, state::AppState, ui::help_text};
use serde_json::Value;

pub async fn handle_dm_message(s: AppState, chat: i64, msg: &Value) {
    // Defense-in-depth owner gate (router parity): direct callers must
    // not bypass auth silently — the router is not the only entry point.
    if let Some(from) = msg["from"]["id"].as_i64()
        && !s.cfg.owners.contains(&from)
    {
        println!("[dm] ignoring non-owner message");
        return;
    }
    let caption = msg["text"]
        .as_str()
        .or_else(|| msg["caption"].as_str())
        .unwrap_or("")
        .trim();
    // Photos prompt with the image attached (downloaded first —
    // caption-only would silently drop the image). Command captions
    // skip the fetch (the command serves imageless — downloading first
    // would burn the sequential pump to discard it). No photo and no
    // text: nothing to serve, stay silent (Telegram never delivers
    // truly empty text updates anyway).
    let photo_id = crate::telegram::photo::largest_file_id(&msg["photo"]);
    if photo_id.is_none() && caption.is_empty() {
        return;
    }
    // Photo + command caption: the command runs as text below, so the
    // image would vanish silently without this note (fail-visible —
    // escapes like /cancel still run, never blocked by an attachment).
    if photo_id.is_some() && super::photo::is_command_caption(caption) {
        s.tg.send_msg(chat, None, crate::ui::PHOTO_CMD_SKIPPED, None)
            .await;
    }
    let text = match photo_id {
        Some(fid) if !super::photo::is_command_caption(caption) => {
            match super::photo::fetch_prompt(&s, chat, None, &fid, caption).await {
                Some(prompt) => prompt,
                None => return,
            }
        }
        _ => caption.to_string(),
    };
    let text = text.as_str();
    // Message bodies stay out of the log (DMs can carry pasted
    // secrets); length suffices for traffic forensics.
    println!("[dm] message ({} chars)", text.chars().count());

    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = super::forum::bare_cmd(raw_cmd);

    let reply_pane: Option<String> = match msg["reply_to_message"]["message_id"].as_i64() {
        Some(rid) => s.targets.lock().await.get(&(chat, rid)).cloned(),
        None => None,
    };

    // /start, /help and /cancel precede armed waiters on purpose:
    // they are the escape hatches out of a stuck waiter (the waiter
    // stays armed otherwise, by design — the NEXT message serves it).
    if cmd == "/start" || cmd == "/help" {
        s.tg.send_msg(chat, None, help_text(), None).await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, None));
        s.runwait.lock().await.remove(&(chat, None));
        s.typewait.lock().await.remove(&(chat, None));
        // Scoped like General (never a silent global nuke).
        let msg = s.cancel_scoped(arg).await;
        s.tg.send_msg(chat, None, &msg, None).await;
        return;
    }

    // Never-stuck escapes precede waiters (like /cancel): a literal
    // "/esc" must dismiss, never become typed input. Rows are fetched
    // here so target resolution works before the main fetch below.
    if cmd == "/card" || cmd == "/esc" {
        let rows = match list_agents(&s.cfg.socket).await {
            Ok(r) => r,
            Err(e) => {
                // Fail-closed single source: herdr error text stays in the
                // log (redacted), never in the chat (socket paths leak $HOME).
                eprintln!(
                    "[dm] list_agents failed: {}",
                    s.tg.redact(&crate::types::mask_home(&e.to_string()))
                );
                s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return;
            }
        };
        if cmd == "/card" {
            super::escape::handle_card_dm(&s, chat, &rows, arg, &reply_pane).await;
        } else {
            super::escape::handle_esc_dm(&s, chat, &rows, arg, &reply_pane).await;
        }
        return;
    }

    // Armed waiters consume the message before any routing: run/key
    // waiters via the shared helper (shell panes need pane keys, not
    // agent keys), then the typed-answer waiter — a "/" answer belongs
    // to the waiting prompt, never to unknown-command.
    if super::tap::consume_runkey(&s, chat, None, text).await {
        return;
    }

    if super::dm_typewait::handle_typewait(&s, chat, text).await {
        return;
    }

    let rows = match list_agents(&s.cfg.socket).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[dm] list_agents failed: {}",
                s.tg.redact(&crate::types::mask_home(&e.to_string()))
            );
            s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return;
        }
    };

    if cmd == "/agents" {
        super::agents::show_panel(&s, chat, None).await;
        return;
    }

    if cmd == "/transient" {
        super::transient::handle_transient(&s, chat, None, arg).await;
        return;
    }

    if cmd == "/keys" {
        super::dm_info::handle_keys(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/read" || cmd == "/output" {
        super::dm_info::handle_read(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/quit" {
        super::dm_lifecycle::handle_quit(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/kill" {
        super::dm_lifecycle::handle_kill(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/shell" {
        super::dm_lifecycle::handle_shell(&s, chat, &rows, arg).await;
        return;
    }

    if cmd == "/pane" {
        super::dm_lifecycle::handle_pane(&s, chat, arg).await;
        return;
    }

    if cmd == "/split" {
        super::dm_lifecycle::handle_split(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/space" {
        super::dm_lifecycle::handle_space(&s, chat, arg).await;
        return;
    }

    if cmd == "/spawn" {
        super::agents::spawn_with_arg(&s, chat, None, arg).await;
        return;
    }

    if cmd == "/model" {
        super::dm_model::handle_model(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/status" {
        super::dm_status::handle_status(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/history" {
        // Target + count like /read (`/history w8:p1 50`, `/history 50`,
        // `/history w8:p1`): an explicit but unknown target errors — it
        // must never answer for a different agent. Pane ids/kinds are
        // never bare numbers, so a numeric word that resolves as a
        // target stays a target.
        let (target_arg, n) = match arg.rsplit_once(char::is_whitespace) {
            Some((head, tail)) => match tail.parse::<i64>() {
                Ok(count) if resolve_target(&rows, Some(arg)).is_none() => {
                    let head = head.trim();
                    let count = count.clamp(1, crate::state::history::HISTORY_CAP as i64) as u32;
                    (if head.is_empty() { None } else { Some(head) }, count)
                }
                _ => (Some(arg), 5),
            },
            None => match arg.parse::<i64>() {
                Ok(count) if resolve_target(&rows, Some(arg)).is_none() => (
                    None,
                    count.clamp(1, crate::state::history::HISTORY_CAP as i64) as u32,
                ),
                _ => (if arg.is_empty() { None } else { Some(arg) }, 5),
            },
        };
        if target_arg.is_none() && super::target::unmatched_reply(&rows, &reply_pane) {
            s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                .await;
            return;
        }
        let mut pane = match resolve_target(&rows, target_arg) {
            Some(r) => Some(r.pane),
            None if target_arg.is_some() => {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
                return;
            }
            None => reply_pane
                .as_deref()
                .and_then(|p| rows.iter().find(|r| r.pane == p).map(|r| r.pane.clone())),
        };
        if pane.is_none() {
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
                    pane = resolve_target(&rows, Some("")).map(|r| r.pane);
                }
            }
        }
        match pane {
            Some(pane) => {
                crate::state::history::send_history(&s, chat, None, &pane, n as usize).await;
            }
            None => {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
            }
        }
        return;
    }

    // Parity with General: bare resets all topics (paced), a target
    // resets one. Forum-only underneath — DM-only answers gracefully.
    if cmd == "/reset" {
        let s2 = s.clone();
        let target = arg.to_string();
        if !target.is_empty() {
            super::reset::spawn_single_topic_reset(&s2, chat, None, target);
        } else {
            tokio::spawn(async move { super::reset::run_paced_reset(&s2, chat, None).await });
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_COMMAND, None)
            .await;
        return;
    }

    super::dm_prompt::handle_bare_prompt(&s, chat, &rows, text, reply_pane).await;
}
