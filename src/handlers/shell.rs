/// Shell-pane CLI: a topic whose pane holds no agent is a terminal.
/// Bare messages run as shell commands, `/quit` drops the agent to a
/// shell (idle only — never nuke real work), and typing `opencode`
/// re-enters naturally (it's just another command). Mode signal is the
/// topic icon (shell badge) plus the `💲` card/menus glyph.
use tokio::time::{Duration, sleep};

use crate::{
    handlers::forum::bare_cmd,
    herdr::client::{
        get_agent, list_panes, read_shell_output, send_agent_keys, send_pane_keys,
        send_pane_text,
    },
    state::AppState,
    ui::{shell_help_text, tail_fit},
};

/// Pure reply body so tests cover the shape without I/O.
pub fn format_shell_reply(cmd: &str, output: &str) -> String {
    let body = if output.trim().is_empty() {
        "(no output)".to_string()
    } else {
        output.trim().to_string()
    };
    format!("$ {cmd}\n{body}")
}

/// Run one shell line in `pane` and report its tail. Shells echo + execute
/// fast; 2.5s covers the common case — longer jobs are re-read with /read.
pub async fn run_shell_cmd(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, cmd: &str) {
    s.set_focus(pane).await;
    if let Err(e) = send_pane_text(&s.cfg.socket, pane, cmd).await {
        s.tg
            .send_msg(chat, thread, &format!("⚠️ pane gone or unreachable: {e}"), None)
            .await;
        return;
    }
    sleep(Duration::from_millis(2500)).await;
    let lines = read_shell_output(&s.cfg.socket, pane, 60)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>();
    let tail = tail_fit(&lines, 3500);
    let mid = s.tg.send_msg(chat, thread, &format_shell_reply(cmd, &tail), None).await;
    s.remember(chat, mid, pane).await;
}

/// Drop the pane's agent to a shell. Idle/done only: quitting a working
/// or blocked agent would destroy real work or answer a dialog blindly.
pub async fn quit_to_shell(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let agent = match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => a,
        Err(_) => {
            // Dead pane (topic lingering) vs live shell — only the latter
            // gets the shell card.
            if !list_panes(&s.cfg.socket).await.unwrap_or_default().contains(&pane.to_string()) {
                s.tg.send_msg(chat, thread, &format!("⚠️ pane {pane} is gone"), None).await;
                return;
            }
            let mid = s
                .tg
                .send_msg(chat, thread, &shell_card_text(pane), None)
                .await;
            s.remember(chat, mid, pane).await;
            s.set_focus(pane).await;
            return;
        }
    };
    if !matches!(agent.status.as_str(), "idle" | "done") {
        s.tg
            .send_msg(
                chat,
                thread,
                &format!("⛔ {} is {} — wait for idle (never quit live work)", pane, agent.status),
                None,
            )
            .await;
        return;
    }
    s.typewait.lock().await.remove(&chat);
    if send_agent_keys(&s.cfg.socket, pane, &["ctrl+c"]).await.is_err() {
        s.tg.send_msg(chat, thread, "⚠️ quit keys failed — quit on the PC", None).await;
        return;
    }
    // Confirm herdr sees a shell (agent_not_found) before claiming it.
    let mut shelled = false;
    for _ in 0..4 {
        sleep(Duration::from_millis(1500)).await;
        if get_agent(&s.cfg.socket, pane).await.is_err() {
            shelled = true;
            break;
        }
    }
    if !shelled {
        s.tg
            .send_msg(chat, thread, &format!("⚠️ still in {} — try again or quit on the PC", agent.kind), None)
            .await;
        return;
    }
    s.cancel_jobs_for(pane).await;
    // Badge shell NOW (not on the next watchdog cycle) so a fast
    // re-enter still hushes correctly in the notifier.
    s.status.lock().await.insert(pane.to_string(), "shell".to_string());
    s.topics.mark_shell(pane).await;
    let mid = s.tg.send_msg(chat, thread, &shell_card_text(pane), None).await;
    s.remember(chat, mid, pane).await;
    s.set_focus(pane).await;
}

/// DM fallback: reply/focus may point at a shell pane (invisible to
/// agent.list) — run it as a command instead of asking "who?".
pub async fn run_shell_fallback(s: &AppState, chat: i64, reply: Option<String>, text: &str) {
    let pane = match reply {
        Some(p) => Some(p),
        None => s.get_focus().await,
    };
    match pane {
        Some(p) => run_shell_cmd(s, chat, None, &p, text).await,
        None => {
            s.tg.send_msg(chat, None, "who? tap an agent in /agents, or reply to its last message", None).await;
        }
    }
}

pub fn shell_card_text(pane: &str) -> String {
    format!("💲 shell [{pane}]\ntype any shell command — or `opencode` to return.")
}

/// Message routing for agentless-topic panes: shell command set.
/// Bare text runs as a command (`opencode` re-enters by itself).
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
        s.tg.send_msg(chat, Some(thread_id), &shell_card_text(pane), None).await;
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
    run_shell_cmd(&s, chat, Some(thread_id), pane, text).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_shell_reply() {
        assert_eq!(
            format_shell_reply("pwd", "/Users/adem/projects").as_str(),
            "$ pwd\n/Users/adem/projects"
        );
        assert_eq!(format_shell_reply("true", "  \n ").as_str(), "$ true\n(no output)");
    }

    #[test]
    fn test_shell_card_text() {
        let t = shell_card_text("w1:p1");
        assert!(t.contains("w1:p1"));
        assert!(t.contains("opencode"));
    }
}
