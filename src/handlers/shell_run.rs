use super::shell_common::{await_shell_settle, format_shell_reply, shell_snapshot};
use crate::{
    herdr::client::{create_tab, list_workspaces, send_pane_input},
    state::AppState,
    ui::{pane_output_kb, tail_fit, ws_label},
};

/// Run one shell line in `pane` and report its tail once the shell
/// settles (see `await_shell_settle`).
pub async fn run_shell_cmd(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, cmd: &str) {
    let before = shell_snapshot(s, pane).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, pane, cmd).await {
        s.tg.send_msg(
            chat,
            thread,
            &format!("⚠️ pane gone or unreachable: {e}"),
            None,
        )
        .await;
        return;
    }
    // Focus only after a live send: focusing a corpse re-arms every next
    // bare message into the void (stale-focus loop).
    s.set_focus(pane).await;
    // Durable intent: a restart mid-settle recovers the tail instead of
    // eating the reply (boot posts it once, then clears).
    s.remember_pending(pane, chat, thread, cmd).await;
    let (out, settled) = await_shell_settle(s, pane, &before).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work.
    if !s.pending.lock().await.contains_key(pane) {
        return;
    }
    let lines = out
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>();
    let mut reply = format_shell_reply(cmd, &tail_fit(&lines, 3500));
    if !settled {
        reply.push_str("\n⏳ still running — output above may grow; `/read` for more.");
    }
    let mid = s.tg.send_msg(chat, thread, &reply, None).await;
    s.remember(chat, mid, pane).await;
    // Delivery-tracked: on Telegram failure the intent survives for
    // boot-recover instead of eating the reply.
    if mid.is_some() {
        s.clear_pending(pane).await;
    }
}

/// Workspace shell-run (⌨️ run cmd button): fresh tab in `ws`, run one
/// command, report output with a refresh button. The tab gets a topic,
/// shell status and focus like any shell — otherwise every run leaks an
/// orphan tab nobody can see.
pub async fn handle_run_command(s: &AppState, chat: i64, thread: Option<i64>, ws: &str, cmd: &str) {
    let pane = match create_tab(&s.cfg.socket, ws).await {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            s.tg.send_msg(chat, thread, "⚠️ tab.create returned no pane", None)
                .await;
            return;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let space = ws_label(&spaces, ws).to_string();
    s.topics.sync_topic(&pane, "shell", &space).await;
    s.status
        .lock()
        .await
        .insert(pane.clone(), "shell".to_string());
    // Focus only after a live send (same rule as run_shell_cmd): a fresh
    // tab whose first send fails must not pin focus on the orphan.
    // Fresh shells start slow (rc files, version managers) — settle first.
    let before = shell_snapshot(s, &pane).await;
    let cmd = cmd.trim();
    s.tg.send_msg(
        chat,
        thread,
        &format!("⏳ running in {ws} [{pane}]\n$ {cmd}"),
        None,
    )
    .await;
    if let Err(e) = send_pane_input(&s.cfg.socket, &pane, cmd).await {
        s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
        return;
    }
    s.set_focus(&pane).await;
    s.remember_pending(&pane, chat, thread, cmd).await;
    let (out, settled) = await_shell_settle(s, &pane, &before).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work.
    if !s.pending.lock().await.contains_key(&pane) {
        return;
    }
    let mut body = if out.trim().is_empty() {
        "(no output yet)".into()
    } else {
        out
    };
    if !settled {
        body.push_str("\n⏳ still running — output above may grow; `/read` for more.");
    }
    let mid =
        s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(&pane)))
            .await;
    s.remember(chat, mid, &pane).await;
    // Delivery-tracked: on Telegram failure the intent survives for
    // boot-recover instead of eating the reply.
    if mid.is_some() {
        s.clear_pending(&pane).await;
    }
}
