use crate::{herdr::client::read_shell_output, jobs::stream::delta, state::AppState, ui::tail_fit};
use serde_json::Value;
use tokio::time::{Duration, sleep};

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
pub(crate) async fn shell_snapshot(s: &AppState, pane: &str) -> String {
    read_shell_output(&s.cfg.socket, pane, 60)
        .await
        .unwrap_or_default()
}

/// Wait for the shell to settle after submitting: poll until two
/// consecutive reads agree AND differ from the pre-send screen (or ~15s).
/// A fixed sleep races slow shell startups (pyenv rehash etc.) and slow
/// commands — the read then catches the typed echo with no output yet.
/// Returns whether the screen stabilized: callers must say so when it
/// did not, never present partial output as final. Aborts early when the
/// pane's pending intent vanishes: /cancel then waits out at most one
/// sleep plus one in-flight herdr read (no hard 1s bound — reads can
/// take 30s+, but the abort itself adds no extra budget). The early
/// return carries whatever was last read (empty on the first round);
/// callers gate on the intent anyway, so it always stays silent.
pub(crate) async fn await_shell_settle(s: &AppState, pane: &str, before: &str) -> (String, bool) {
    let mut last = String::new();
    let mut stable = 0u32;
    let mut cur = String::new();
    for _ in 0..15 {
        sleep(Duration::from_secs(1)).await;
        // Prompt abort, not a settle verdict: the caller gates on the
        // intent anyway — this just skips dead sleeping.
        if !s.pending.lock().await.contains_key(pane) {
            return (cur, false);
        }
        cur = shell_snapshot(s, pane).await;
        if cur != before && cur == last {
            stable += 1;
            if stable >= 2 {
                return (cur, true);
            }
        } else {
            stable = 0;
        }
        last = cur.clone();
    }
    (cur, false)
}

pub fn shell_card_text(pane: &str) -> String {
    format!("💲 shell [{pane}]\ntype any shell command — or `opencode` to return.")
}

/// Follow-up budget after the first unsettled card: ~20 rounds × ~15s
/// ≈ 5 minutes. Never-ending runs (servers, watchers) stop here with a
/// still-running card; `/read` covers the rest.
pub(crate) const SHELL_FOLLOW_UP_ROUNDS: u32 = 20;

/// How to close a shell result card.
pub(crate) enum ShellNote {
    /// Settled: the tail is final, no footer.
    Final,
    /// First card, still running: a completion card follows.
    Following,
    /// Follow-up, now settled.
    Finished,
    /// Budget exhausted, still running.
    StillRunning,
}

/// Pure result-card body so tests cover the shape without I/O.
pub fn shell_result_text(cmd: &str, out: &str, note: ShellNote) -> String {
    let lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    let mut reply = format_shell_reply(cmd, &tail_fit(&lines, 3500));
    match note {
        ShellNote::Final => {}
        ShellNote::Following => {
            reply.push_str("\n⏳ still running — following up when it settles…")
        }
        ShellNote::Finished => reply.push_str("\n✅ finished."),
        ShellNote::StillRunning => {
            reply.push_str("\n⏳ still running — output above may grow; `/read` for more.")
        }
    }
    reply
}

/// Pure retire decision for reconcile's agent→shell branch (no I/O).
/// Shell intents and agent intents share one `pending` map, so a blind
/// retire eats an active shell command's intent on every watchdog tick
/// — long shell runs would post nothing after their start card.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShellReuse {
    /// Already shell, no job: active shell work — touch nothing.
    Ignore,
    /// Already shell with a stale watcher: kill the watcher only,
    /// preserving the shell command's pending intent.
    CancelJob,
    /// Fresh agent→shell flip with owed work: full retire + quit notice.
    RetireVanished,
}

pub(crate) fn classify_shell_reuse(was_shell: bool, owed: bool, job: bool) -> ShellReuse {
    if was_shell {
        if job {
            ShellReuse::CancelJob
        } else {
            ShellReuse::Ignore
        }
    } else if owed || job {
        ShellReuse::RetireVanished
    } else {
        ShellReuse::Ignore
    }
}

/// Settle one shell command and report it, following up when a long run
/// outlasts the first settle: silent polls until it finishes (bounded),
/// then one completion card with the fresh tail — never partial-card
/// spam, never silence after the start card. Delivery-tracked and
/// cancel-aware: a failed send keeps the intent for boot-recover, a
/// cleared intent (/cancel) stays silent.
pub(crate) async fn settle_report_shell(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    cmd: &str,
    before: &str,
    kb: Option<Value>,
) {
    let (out, settled) = await_shell_settle(s, pane, before).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work.
    if !s.pending_matches(pane, chat, thread, cmd).await {
        return;
    }
    let first = if settled {
        ShellNote::Final
    } else {
        ShellNote::Following
    };
    let mid =
        s.tg.send_msg(
            chat,
            thread,
            &shell_result_text(cmd, &out, first),
            kb.clone(),
        )
        .await;
    s.remember(chat, mid, pane).await;
    if mid.is_none() {
        // Delivery-tracked: the intent survives for boot-recover instead
        // of eating the reply. No follow-up without a first card.
        return;
    }
    let sent_lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    let mut settle_base = out;
    if settled {
        s.clear_pending(pane).await;
        return;
    }
    for _ in 0..SHELL_FOLLOW_UP_ROUNDS {
        let (next, done) = await_shell_settle(s, pane, &settle_base).await;
        settle_base = next.clone();
        if !s.pending_matches(pane, chat, thread, cmd).await {
            return;
        }
        if !done {
            continue; // silent poll: no partial-card spam
        }
        let new_lines: Vec<String> = next.lines().map(|l| l.trim_end().to_string()).collect();
        let fresh = delta(&new_lines, &sent_lines).to_vec();
        let body = if fresh.iter().all(|l| l.trim().is_empty()) {
            format!("$ {cmd}\n✅ finished — no further output.")
        } else {
            shell_result_text(cmd, &fresh.join("\n"), ShellNote::Finished)
        };
        let mid2 = s.tg.send_msg(chat, thread, &body, kb.clone()).await;
        s.remember(chat, mid2, pane).await;
        if mid2.is_some() {
            s.clear_pending(pane).await;
        }
        return;
    }
    // Budget exhausted, still running: last fresh tail (or a short note
    // when nothing new arrived all budget) + `/read` pointer.
    if !s.pending_matches(pane, chat, thread, cmd).await {
        return;
    }
    let new_lines: Vec<String> = settle_base
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let fresh = delta(&new_lines, &sent_lines);
    let body = if fresh.iter().all(|l| l.trim().is_empty()) {
        format!("$ {cmd}\n⏳ still running — `/read` for more.")
    } else {
        shell_result_text(cmd, &fresh.join("\n"), ShellNote::StillRunning)
    };
    let mid3 = s.tg.send_msg(chat, thread, &body, kb).await;
    s.remember(chat, mid3, pane).await;
    if mid3.is_some() {
        s.clear_pending(pane).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_shell_reply() {
        assert_eq!(
            format_shell_reply("pwd", "/home/user/projects").as_str(),
            "$ pwd\n/home/user/projects"
        );
        assert_eq!(
            format_shell_reply("true", "  \n ").as_str(),
            "$ true\n(no output)"
        );
    }

    #[test]
    fn test_shell_card_text() {
        let t = shell_card_text("w1:p1");
        assert!(t.contains("w1:p1"));
        assert!(t.contains("opencode"));
    }

    #[test]
    fn test_shell_result_text_notes() {
        let out = "line1\nline2";
        let final_card = shell_result_text("make build", out, ShellNote::Final);
        assert!(final_card.contains("$ make build"));
        assert!(final_card.contains("line2"));
        assert!(!final_card.contains("⏳") && !final_card.contains("✅"));
        assert!(
            shell_result_text("make build", out, ShellNote::Following).contains("following up")
        );
        assert!(shell_result_text("make build", out, ShellNote::Finished).contains("✅ finished"));
        assert!(shell_result_text("make build", out, ShellNote::StillRunning).contains("/read"));
        // Empty output never posts a bare prompt line.
        assert!(shell_result_text("true", "  \n ", ShellNote::Final).contains("(no output)"));
    }

    #[test]
    fn test_classify_shell_reuse() {
        use ShellReuse::*;
        // Live shell work: never touch (the long-run intent-eat bug).
        assert_eq!(classify_shell_reuse(true, true, false), Ignore);
        // Stale watcher on a live shell: job only, intent preserved.
        assert_eq!(classify_shell_reuse(true, true, true), CancelJob);
        assert_eq!(classify_shell_reuse(true, false, true), CancelJob);
        // Fresh agent→shell flip: full retire + quit notice.
        assert_eq!(classify_shell_reuse(false, true, false), RetireVanished);
        assert_eq!(classify_shell_reuse(false, false, true), RetireVanished);
        assert_eq!(classify_shell_reuse(false, true, true), RetireVanished);
        // Nothing owed: ignore.
        assert_eq!(classify_shell_reuse(false, false, false), Ignore);
        assert_eq!(classify_shell_reuse(true, false, false), Ignore);
    }
}
