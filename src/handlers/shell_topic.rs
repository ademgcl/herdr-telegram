/// Message routing for agentless-topic panes: the shell command set.
/// Bare text runs as a command (any agent binary re-enters by itself). Split
/// from shell.rs (ops) under the 300-line file cap.
use crate::{
    handlers::forum::bare_cmd,
    herdr::client::{read_shell_output, send_pane_keys},
    state::AppState,
    ui::shell_help_text,
};

pub async fn handle_shell_topic(s: AppState, chat: i64, thread_id: i64, pane: &str, text: &str) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = bare_cmd(raw_cmd);

    // Shell topics never consume typewaits (no blocked dialogs here): a
    // waiter stranded on this thread (stale/crafted B:type) would sit
    // forever, so drop it on entry.
    s.typewait.lock().await.remove(&(chat, Some(thread_id)));

    if cmd == "/help" {
        s.tg.send_msg(chat, Some(thread_id), &shell_help_text(pane), None)
            .await;
        return;
    }
    if cmd == "/reset" {
        let _ = super::reset::run_single_topic_reset(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if cmd == "/quit" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            "already in shell — type any command.",
            None,
        )
        .await;
        return;
    }
    if cmd == "/kill" {
        super::kill::ask_kill(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if cmd == "/split" {
        let dir = match arg {
            "" | "right" | "down" => arg,
            _ => {
                s.tg.send_msg(chat, Some(thread_id), "usage: `/split [right|down]` — bare picks the longer side", None)
                    .await;
                return;
            }
        };
        super::shell::open_split(&s, chat, Some(thread_id), pane, dir).await;
        return;
    }
    if cmd == "/pane" {
        super::shell::open_pane_here(&s, chat, Some(thread_id), pane, arg).await;
        return;
    }
    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, Some(thread_id)));
        s.runwait.lock().await.remove(&(chat, Some(thread_id)));
        s.typewait.lock().await.remove(&(chat, Some(thread_id)));
        // Non-empty arg routes via scope (all | pane id); bare cancels
        // this topic's pane (never focus — topics name their pane).
        if arg.split_whitespace().next().is_some() {
            let msg = s.cancel_scoped(arg).await;
            s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
            return;
        }
        let n = s.cancel_jobs_for(pane).await;
        let msg = if n {
            format!("✋ cancelled {pane}")
        } else {
            format!("nothing running for {pane}")
        };
        s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
        return;
    }
    // Never-stuck escapes precede run/key waiters: an armed waiter must
    // never eat /card or /esc as keys or a command.
    if cmd == "/card" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            "this is a shell — nothing to answer.",
            None,
        )
        .await;
        return;
    }
    if cmd == "/esc" {
        super::escape::handle_esc_shell(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if super::tap::consume_runkey(&s, chat, Some(thread_id), text).await {
        return;
    }
    if cmd == "/read" || cmd == "/output" {
        let lines = arg.parse::<u32>().map(|n| n.clamp(1, 400)).unwrap_or(60);
        match read_shell_output(&s.cfg.socket, pane, lines).await {
            Ok(out) => {
                let body = if out.trim().is_empty() {
                    "(no output)".into()
                } else {
                    out
                };
                s.tg.send_msg(chat, Some(thread_id), &body, None).await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None)
                    .await;
            }
        }
        return;
    }
    if cmd == "/history" {
        crate::state::history::send_history(&s, chat, Some(thread_id), pane, arg).await;
        return;
    }
    if cmd == "/keys" {
        if arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), "usage: `/keys y enter`", None)
                .await;
            return;
        }
        let keys: Vec<&str> = arg.split_whitespace().collect();
        match send_pane_keys(&s.cfg.socket, pane, &keys).await {
            Ok(_) => {
                s.tg.send_msg(chat, Some(thread_id), "⌨️ keys sent", None)
                    .await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None)
                    .await;
            }
        }
        return;
    }
    if cmd == "/status" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            &super::shell::shell_card_text(pane),
            None,
        )
        .await;
        return;
    }
    if cmd == "/model" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            "no agent here — run one (`opencode`, `claude`, …) to start it.",
            None,
        )
        .await;
        return;
    }
    if cmd.starts_with('/') {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            "unknown shell command — `/help` lists them.",
            None,
        )
        .await;
        return;
    }
    // Bare message in a shell topic -> run it.
    super::shell::run_shell_cmd(&s, chat, Some(thread_id), pane, text).await;
}
