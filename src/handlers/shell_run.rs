use super::shell_common::{ShellSettle, settle_report_shell, shell_snapshot};
use super::shell_validate::validate_shell_cmd;
use crate::{
    herdr::client::{create_tab, list_workspaces, send_pane_input},
    state::AppState,
    ui::{pane_output_kb, ws_label},
};

/// Run one shell line in `pane` and report its tail once the shell
/// settles — plus a completion follow-up when a long run outlasts the
/// first settle (see `settle_report_shell`).
pub async fn run_shell_cmd(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, cmd: &str) {
    // Fail-closed before any RPC: shared cap with the fresh-tab path.
    let cmd = match validate_shell_cmd(cmd) {
        Ok(t) => t,
        Err(msg) => {
            s.tg.send_msg(chat, thread, msg, None).await;
            return;
        }
    };
    let before = shell_snapshot(s, pane).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, pane, cmd).await {
        // Chat stays static (herdr errors carry socket/cwd paths — no
        // username leak); detail goes to the redacted log only.
        // Single source (UNKNOWN_TARGET): dup'd "gone" literals re-drift.
        eprintln!("[shell] send to {pane} failed: {}", s.tg.redact(&crate::types::mask_home(&e.to_string())));
        s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_TARGET, None).await;
        return;
    }
    // Focus only after a live send: focusing a corpse re-arms every next
    // bare message into the void (stale-focus loop).
    s.set_focus(pane).await;
    // Durable intent: a restart mid-settle recovers the tail instead of
    // eating the reply (boot posts it once, then clears).
    s.remember_pending(pane, chat, thread, cmd).await;
    s.push_history(pane, cmd).await;
    // Shell→agent flip (`opencode` typed at the prompt) re-icons in
    // seconds via the spawned watch, not next watchdog tick.
    super::shell_lifecycle::spawn_flip_watch(s, pane);
    // Detached: the settle budget runs ~5.5 min and the Telegram pump is
    // sequential — awaiting it here would head-of-line-block every pane
    // (watchdog ≤60s violated globally, /cancel deadened for the very
    // command stalling). Generation-guarded inside (epoch + pending
    // match before every post, clear-if-matches for clears): a racing
    // resubmit owns the slot and overlapping settles retire silently.
    let epoch = s.bump_shell_epoch(pane).await;
    let (s2, pane_o, cmd_o, before_o) =
        (s.clone(), pane.to_string(), cmd.to_string(), before.clone());
    tokio::spawn(async move {
        settle_report_shell(
            &s2,
            ShellSettle {
                chat,
                thread,
                pane: pane_o,
                cmd: cmd_o,
                before: before_o,
                kb: None,
                epoch,
            },
        )
        .await;
    });
}

/// Workspace shell-run (⌨️ run cmd button): fresh tab in `ws`, run one
/// command, report output with a refresh button. The tab gets a topic,
/// shell status and focus like any shell — otherwise every run leaks an
/// orphan tab nobody can see.
pub async fn handle_run_command(s: &AppState, chat: i64, thread: Option<i64>, ws: &str, cmd: &str) {
    // Fail-closed before minting: whitespace-only input must never mint a
    // tab + topic + pending intent + settle loop for nothing. Shared cap
    // with the existing-pane path (single source, never drift).
    let cmd = match validate_shell_cmd(cmd) {
        Ok(t) => t,
        Err(msg) => {
            s.tg.send_msg(chat, thread, msg, None).await;
            return;
        }
    };
    let pane = match create_tab(&s.cfg.socket, ws).await {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            s.tg.send_msg(chat, thread, "⚠️ tab.create returned no pane", None)
                .await;
            return;
        }
        Err(e) => {
            // Chat stays static (herdr errors carry socket/cwd paths);
            // detail goes to the redacted log only (run_shell_cmd parity).
            eprintln!("[shell] tab.create failed: {}", s.tg.redact(&crate::types::mask_home(&e.to_string())));
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None).await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let space = ws_label(&spaces, ws).to_string();
    // Fresh tab: retire only on a raced prune (uniform, see agents.rs).
    if s.topics.sync_topic_prune(&pane, "shell", &space).await.1 {
        crate::handlers::dialog::retire_dialog(s, &pane).await;
    }
    s.status
        .lock()
        .await
        .insert(pane.clone(), "shell".to_string());
    // Focus only after a live send (same rule as run_shell_cmd): a fresh
    // tab whose first send fails must not pin focus on the orphan.
    // Fresh shells start slow (rc files, version managers) — settle first.
    // One-command-one-card: the ⏳ posts AFTER a live send, so a failed
    // first send leaves no orphan provisional beside the gone notice.
    let before = shell_snapshot(s, &pane).await;
    if let Err(e) = send_pane_input(&s.cfg.socket, &pane, cmd).await {
        // Fresh tab whose first send fails: static outage ack (never raw
        // herdr paths), no focus pin (run_shell_cmd parity). NOT "gone":
        // the pane was just created — a blip must read as retry, and the
        // topic+status stay for the retry (no orphan: same pane serves it).
        eprintln!("[shell] first send to {pane} failed: {}", s.tg.redact(&crate::types::mask_home(&e.to_string())));
        s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None).await;
        return;
    }
    s.tg.send_msg(
        chat,
        thread,
        &format!("⏳ running in {space} [{pane}]\n$ {cmd}"),
        None,
    )
    .await;
    s.set_focus(&pane).await;
    s.remember_pending(&pane, chat, thread, cmd).await;
    s.push_history(&pane, cmd).await;
    super::shell_lifecycle::spawn_flip_watch(s, &pane);
    // Detached like run_shell_cmd above: same ~5.5 min budget, same
    // sequential-pump head-of-line block, same generation-guarded safety.
    let epoch = s.bump_shell_epoch(&pane).await;
    let (s2, pane_o, cmd_o, before_o, kb) = (
        s.clone(),
        pane.clone(),
        cmd.to_string(),
        before.clone(),
        Some(pane_output_kb(&pane)),
    );
    tokio::spawn(async move {
        settle_report_shell(
            &s2,
            ShellSettle {
                chat,
                thread,
                pane: pane_o,
                cmd: cmd_o,
                before: before_o,
                kb,
                epoch,
            },
        )
        .await;
    });
}
