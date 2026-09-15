use super::shell_common::{await_shell_settle, format_shell_reply, shell_snapshot};
use crate::{
    herdr::client::{create_tab, send_pane_input},
    state::AppState,
    ui::{pane_output_kb, tail_fit},
};

/// Run one shell line in `pane` and report its tail once the shell
/// settles (see `await_shell_settle`).
pub async fn run_shell_cmd(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, cmd: &str) {
    s.set_focus(pane).await;
    let before = shell_snapshot(s, pane).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, pane, cmd).await {
        s.clear_pending(pane).await;
        s.tg.send_msg(
            chat,
            thread,
            &format!("⚠️ pane gone or unreachable: {e}"),
            None,
        )
        .await;
        return;
    }
    // Durable intent: a restart mid-settle recovers the tail instead of
    // eating the reply (boot posts it once, then clears).
    s.remember_pending(pane, chat, thread, cmd).await;
    let out = await_shell_settle(s, pane, &before).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work.
    if !s.pending.lock().await.contains_key(pane) {
        return;
    }
    let lines = out
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>();
    let tail = tail_fit(&lines, 3500);
    let mid =
        s.tg.send_msg(chat, thread, &format_shell_reply(cmd, &tail), None)
            .await;
    s.remember(chat, mid, pane).await;
    s.clear_pending(pane).await;
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
    s.tg.send_msg(
        chat,
        thread,
        &format!("⏳ running in {ws} [{pane}]\n$ {cmd}"),
        None,
    )
    .await;
    if let Err(e) = send_pane_input(&s.cfg.socket, &pane, cmd).await {
        s.clear_pending(&pane).await;
        s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
        return;
    }
    s.remember_pending(&pane, chat, thread, cmd).await;
    let out = await_shell_settle(s, &pane, &before).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work.
    if !s.pending.lock().await.contains_key(&pane) {
        return;
    }
    let body = if out.trim().is_empty() {
        "(no output yet)".into()
    } else {
        out
    };
    let mid =
        s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(&pane)))
            .await;
    s.remember(chat, mid, &pane).await;
    s.clear_pending(&pane).await;
}
