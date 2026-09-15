/// Shell-pane CLI: a topic whose pane holds no agent is a terminal.
/// Bare messages run as shell commands, `/quit` drops the agent to a
/// shell (idle only — never nuke real work), and typing `opencode`
/// re-enters naturally (it's just another command). Mode signal is the
/// topic icon (shell badge) plus the `💲` card/menus glyph.
use tokio::time::{Duration, sleep};

use crate::{
    herdr::client::{
        create_tab, ensure_tg_space, get_agent, list_panes, list_workspaces,
        read_shell_output, send_agent_keys, send_pane_input,
    },
    herdr::labels::pane_facts,
    state::AppState,
    ui::{pane_output_kb, tail_fit, ws_label},
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

/// Read one shell snapshot (best effort).
async fn shell_snapshot(s: &AppState, pane: &str) -> String {
    read_shell_output(&s.cfg.socket, pane, 60).await.unwrap_or_default()
}

/// Wait for the shell to settle after submitting: poll until two
/// consecutive reads agree AND differ from the pre-send screen (or ~10s).
/// A fixed sleep races slow shell startups (pyenv rehash etc.) and slow
/// commands — the read then catches the typed echo with no output yet.
async fn await_shell_settle(s: &AppState, pane: &str, before: &str) -> String {
    let mut last = String::new();
    let mut stable = 0u32;
    let mut cur = String::new();
    for _ in 0..10 {
        sleep(Duration::from_secs(1)).await;
        cur = shell_snapshot(s, pane).await;
        if cur != before && cur == last {
            stable += 1;
            if stable >= 2 {
                break;
            }
        } else {
            stable = 0;
        }
        last = cur.clone();
    }
    cur
}

/// Run one shell line in `pane` and report its tail once the shell
/// settles (see `await_shell_settle`).
pub async fn run_shell_cmd(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, cmd: &str) {
    s.set_focus(pane).await;
    // Durable intent: a restart mid-settle recovers the tail instead of
    // eating the reply (boot posts it once, then clears).
    s.remember_pending(pane, chat, thread, cmd).await;
    let before = shell_snapshot(s, pane).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, pane, cmd).await {
        s.clear_pending(pane).await;
        s.tg
            .send_msg(chat, thread, &format!("⚠️ pane gone or unreachable: {e}"), None)
            .await;
        return;
    }
    let out = await_shell_settle(s, pane, &before).await;
    let lines = out
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>();
    let tail = tail_fit(&lines, 3500);
    let mid = s.tg.send_msg(chat, thread, &format_shell_reply(cmd, &tail), None).await;
    s.remember(chat, mid, pane).await;
    s.clear_pending(pane).await;
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
    s.typewait.lock().await.remove(&(chat, thread));
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
    s.clear_pane(pane).await;
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

/// Open a fresh shell pane: new tab in `ws` (or the tg space), topic
/// badged shell, ready for commands. `ws` is a workspace id.
pub async fn open_shell(s: &AppState, chat: i64, thread: Option<i64>, ws: Option<&str>) {
    let ws_id = match ws {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => match ensure_tg_space(&s.cfg.socket).await {
            Ok(id) => id,
            Err(e) => {
                s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
                return;
            }
        },
    };
    let pane = match create_tab(&s.cfg.socket, &ws_id).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let space = ws_label(&spaces, &ws_id).to_string();
    // Kind "shell" mints an sh<n> tag; icon goes straight to shell.
    s.topics.sync_topic(&pane, "shell", &space, "shell").await;
    s.status.lock().await.insert(pane.clone(), "shell".to_string());
    let mid = s.tg.send_msg(chat, thread, &shell_card_text(&pane), None).await;
    s.remember(chat, mid, &pane).await;
    s.set_focus(&pane).await;
}

/// Split the pane sideways in the SAME tab: sibling shell pane + its own
/// topic, named/provisioned like any shell. `dir` is right|down.
pub async fn open_split(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, dir: &str) {
    let new = match crate::herdr::client::split_pane(&s.cfg.socket, pane, dir).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ split failed: {e}"), None).await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let ws_id = pane_facts(&s.cfg.socket).await.ok().and_then(|m| m.get(pane).map(|f| f.ws.clone())).unwrap_or_default();
    let space = ws_label(&spaces, &ws_id).to_string();
    let space = if space.is_empty() { pane } else { &space };
    s.topics.sync_topic(&new, "shell", space, "shell").await;
    s.status.lock().await.insert(new.clone(), "shell".to_string());
    let mid = s.tg.send_msg(chat, thread, &shell_card_text(&new), None).await;
    s.remember(chat, mid, &new).await;
    s.set_focus(&new).await;
}

pub fn shell_card_text(pane: &str) -> String {
    format!("💲 shell [{pane}]\ntype any shell command — or `opencode` to return.")
}

/// Workspace shell-run (⌨️ run cmd button): fresh tab in `ws`, run one
/// command, report output with a refresh button.
pub async fn handle_run_command(s: &AppState, chat: i64, thread: Option<i64>, ws: &str, cmd: &str) {
    let pane = match create_tab(&s.cfg.socket, ws).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
            return;
        }
    };
    // Fresh shells start slow (rc files, version managers) — settle first.
    let before = shell_snapshot(s, &pane).await;
    let cmd = cmd.trim();
    s.remember_pending(&pane, chat, thread, cmd).await;
    s.tg.send_msg(chat, thread, &format!("⏳ running in {ws} [{pane}]\n$ {cmd}"), None).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, &pane, cmd).await {
        s.clear_pending(&pane).await;
        s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
        return;
    }
    let out = await_shell_settle(s, &pane, &before).await;
    let body = if out.trim().is_empty() { "(no output yet)".into() } else { out };
    let mid = s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(&pane))).await;
    s.remember(chat, mid, &pane).await;
    s.clear_pending(&pane).await;
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
