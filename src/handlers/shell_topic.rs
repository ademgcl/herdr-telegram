/// Message routing for agentless-topic panes: the shell command set.
/// Bare text runs as a command (`opencode` re-enters by itself). Split
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

    if cmd == "/help" {
        s.tg.send_msg(chat, Some(thread_id), &shell_help_text(pane), None).await;
        return;
    }
    if cmd == "/quit" {
        s.tg.send_msg(chat, Some(thread_id), "already in shell — type any command.", None).await;
        return;
    }
    if cmd == "/kill" {
        super::kill::ask_kill(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if cmd == "/cancel" {
        let n = s.cancel_jobs_for(pane).await;
        let msg = if n { "✋ cancelled pane job" } else { "shell mode — nothing running" };
        s.tg.send_msg(chat, Some(thread_id), msg, None).await;
        return;
    }
    if cmd == "/read" || cmd == "/output" {
        let lines = arg.parse::<u32>().unwrap_or(60);
        match read_shell_output(&s.cfg.socket, pane, lines).await {
            Ok(out) => {
                let body = if out.trim().is_empty() { "(no output)".into() } else { out };
                s.tg.send_msg(chat, Some(thread_id), &body, None).await;
            }
            Err(e) => { s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None).await; }
        }
        return;
    }
    if cmd == "/keys" {
        if arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), "usage: `/keys y enter`", None).await;
            return;
        }
        let keys: Vec<&str> = arg.split_whitespace().collect();
        match send_pane_keys(&s.cfg.socket, pane, &keys).await {
            Ok(_) => { s.tg.send_msg(chat, Some(thread_id), "⌨️ keys sent", None).await; }
            Err(e) => { s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None).await; }
        }
        return;
    }
    if cmd == "/status" {
        s.tg.send_msg(chat, Some(thread_id), &super::shell::shell_card_text(pane), None).await;
        return;
    }
    if cmd == "/model" {
        s.tg.send_msg(chat, Some(thread_id), "no agent here — type `opencode` to start one.", None).await;
        return;
    }
    if cmd.starts_with('/') {
        s.tg.send_msg(chat, Some(thread_id), "unknown shell command — `/help` lists them.", None).await;
        return;
    }
    // Bare message in a shell topic -> run it.
    super::shell::run_shell_cmd(&s, chat, Some(thread_id), pane, text).await;
}
