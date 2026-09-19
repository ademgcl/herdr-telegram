/// Message routing for agentless-topic panes: the shell command set.
/// Bare text runs as a command (any agent binary re-enters by itself). Split
/// from handlers/shell under the 300-line file cap.
use crate::{
    handlers::forum::bare_cmd,
    herdr::client::{read_shell_output, send_pane_keys},
    state::AppState,
    ui::{
        scope_text::{
            READ_CAP, SHELL_READ_DEFAULT, USAGE_HISTORY_TOPIC, USAGE_READ_TOPIC,
            USAGE_RESET_TOPIC, parse_count,
        },
        shell_help_text,
    },
};

pub async fn handle_shell_topic(s: AppState, chat: i64, thread_id: i64, pane: &str, text: &str) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = bare_cmd(raw_cmd);

    // Shell topics never consume typewaits (no blocked dialogs here): a
    // waiter stranded on this thread (stale/crafted B:type) would sit
    // forever, so drop it on entry. Fail-closed: a dropped waiter means
    // the text was typed as a dialog answer across an agent→shell flip —
    // never run it as a shell command blind (no write on ambiguous).
    let had_typewait = s.typewait.lock().await.remove(&(chat, Some(thread_id))).is_some();

    if cmd == "/help" || cmd == "/start" {
        s.tg.send_msg(chat, Some(thread_id), &shell_help_text(pane), None)
            .await;
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
    if had_typewait {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            crate::ui::STALE_TYPEWAIT_SHELL,
            None,
        )
        .await;
        return;
    }
    if super::tap::consume_runkey(&s, chat, Some(thread_id), text).await {
        return;
    }
    // Everything below follows the waiter (see forum_topic): an
    // armed run waiter owns the next message.
    if cmd == "/shell" {
        s.tg
            .send_msg(
                chat,
                Some(thread_id),
                "already in a shell topic — `/pane` for a second shell here.",
                None,
            )
            .await;
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
    // Control plane + reset follow the waiter too.
    if cmd == "/agents" || cmd == "/spawn" {
        super::agents::handle_control(&s, chat, Some(thread_id), cmd, arg).await;
        return;
    }
    if cmd == "/reset" {
        // Own-pane-only like the agent flavor: an arg is refused, never
        // a cross-pane reset on a typo. Spawned: must not stall the pump.
        if !arg.is_empty() {
            s.tg
                .send_msg(chat, Some(thread_id), USAGE_RESET_TOPIC, None)
                .await;
            return;
        }
        super::reset::spawn_single_topic_reset(&s, chat, Some(thread_id), pane.to_string());
        return;
    }
    if cmd == "/read" || cmd == "/output" {
        // Own-pane-only with a count (see forum_topic): pane-shaped args
        // refuse instead of parsing as a count.
        let Some(lines) = parse_count(arg, SHELL_READ_DEFAULT, READ_CAP) else {
            s.tg.send_msg(chat, Some(thread_id), USAGE_READ_TOPIC, None).await;
            return;
        };
        match read_shell_output(&s.cfg.socket, pane, lines).await {
            Ok(out) => {
                let body = if out.trim().is_empty() {
                    crate::ui::NO_OUTPUT.into()
                } else {
                    out
                };
                s.tg.send_msg(chat, Some(thread_id), &body, None).await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None)
                    .await;
            }
        }
        return;
    }
    if cmd == "/history" {
        // Counts only (see forum_topic): foreign text was silently
        // defaulting to this pane's last 5 — refuse instead.
        match parse_count(arg, 5, crate::state::history::HISTORY_CAP as u32) {
            Some(n) => {
                crate::state::history::send_history(&s, chat, Some(thread_id), pane, n as usize)
                    .await;
            }
            None => {
                s.tg.send_msg(chat, Some(thread_id), USAGE_HISTORY_TOPIC, None).await;
            }
        }
        return;
    }
    if cmd == "/keys" {
        if arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), crate::ui::scope_text::USAGE_KEYS_BARE, None)
                .await;
            return;
        }
        // Pane-shaped first tokens refuse (see topic_keys): they were
        // typed as keystrokes into the live shell.
        if let Some(usage) = super::topic_keys::guard_keys_first_token(&s, arg).await {
            s.tg.send_msg(chat, Some(thread_id), &usage, None).await;
            return;
        }
        let keys: Vec<&str> = arg.split_whitespace().collect();
        // Bounded like every keys arm (single source): refuse, never truncate.
        if let Err(msg) = super::shell_validate::validate_keys_len(keys.len()) {
            s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
            return;
        }
        match send_pane_keys(&s.cfg.socket, pane, &keys).await {
            Ok(_) => {
                // Keys may have launched an agent — same instant re-icon.
                super::shell_lifecycle::spawn_flip_watch(&s, pane);
                s.tg.send_msg(chat, Some(thread_id), crate::ui::KEYS_SENT, None)
                    .await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None)
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
