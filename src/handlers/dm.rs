use crate::{herdr::client::list_agents, state::AppState, ui::help_text};
use serde_json::Value;

pub async fn handle_dm_message(s: AppState, chat: i64, msg: &Value) {
    let text = msg["text"]
        .as_str()
        .or_else(|| msg["caption"].as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return;
    }
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
                s.tg.send_msg(chat, None, &format!("⚠️ herdr unreachable: {e}"), None)
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

    if super::dm_prompt::handle_typewait(&s, chat, text).await {
        return;
    }

    let rows = match list_agents(&s.cfg.socket).await {
        Ok(r) => r,
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("⚠️ herdr unreachable: {e}"), None)
                .await;
            return;
        }
    };

    if cmd == "/agents" {
        super::dm_info::handle_agents(&s, chat, &rows).await;
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
        super::dm_lifecycle::handle_spawn(&s, chat, arg).await;
        return;
    }

    if cmd == "/model" {
        super::dm_model::handle_model(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/status" {
        super::dm_info::handle_status(&s, chat, &rows, arg, &reply_pane).await;
        return;
    }

    if cmd == "/history" {
        // Counts only (topics share the rule): a pane-shaped arg was
        // silently dropped to a default-count read of the focus pane.
        // The validated count flows through (never re-parsed downstream).
        let Some(n) = crate::ui::scope_text::parse_count(
            arg,
            5,
            crate::state::history::HISTORY_CAP as u32,
        ) else {
            s.tg
                .send_msg(chat, None, crate::ui::scope_text::USAGE_HISTORY_DM, None)
                .await;
            return;
        };
        match super::target::dm_pane(&s, &rows, "", &reply_pane).await {
            Some(pane) => {
                crate::state::history::send_history(&s, chat, None, &pane, n as usize).await;
            }
            None => {
                s.tg.send_msg(
                    chat,
                    None,
                    "who? reply to an agent card or tap one in /agents",
                    None,
                )
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
            tokio::spawn(async move {
                let _ = super::reset::run_single_topic_reset(&s2, chat, None, &target).await;
            });
        } else {
            tokio::spawn(async move { super::reset::run_paced_reset(&s2, chat, None).await });
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, None, "unknown command — /help", None)
            .await;
        return;
    }

    super::dm_prompt::handle_bare_prompt(&s, chat, &rows, text, reply_pane).await;
}
