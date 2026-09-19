use super::shell_settle::await_shell_settle;
use crate::{herdr::client::get_agent, jobs::stream::delta, state::AppState, ui::tail_fit};
use serde_json::Value;

// Re-exported for the submit paths (`shell_run`, boot-recover): the
// snapshot reader lives in `shell_settle` with its poll-loop consumer.
pub(crate) use super::shell_settle::shell_snapshot;

/// Pure reply body so tests cover the shape without I/O.
pub fn format_shell_reply(cmd: &str, output: &str) -> String {
    let body = if output.trim().is_empty() {
        crate::ui::NO_OUTPUT.to_string()
    } else {
        output.trim().to_string()
    };
    format!("$ {cmd}\n{body}")
}

pub fn shell_card_text(pane: &str) -> String {
    format!("💲 shell [{pane}]\ntype any shell command — or `opencode`, `claude`, … to re-enter.")
}

/// Fresh screen text since the pre-send snapshot: first result cards
/// post only what the command added — the snapshot carries the whole
/// scrollback, and reposting it dumps old output into every reply.
/// Depth-independent (shallow windows delta the same way), so commands
/// never bleed into each other. Screens only append + scroll: the new
/// window opens with a baseline suffix, except the volatile prompt
/// line (replaced by the typed echo once the command runs). Degrades
/// to the trimmed snapshot when nothing aligns (resized redraw).
pub fn fresh_since(out: &str, before: &str) -> String {
    let new: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    let base: Vec<String> = before.lines().map(|l| l.trim_end().to_string()).collect();
    // Whole-baseline suffix first (unchanged screens empty out exactly).
    for j in 0..base.len() {
        if new.len() >= base.len() - j && new[..base.len() - j] == base[j..] {
            return new[base.len() - j..].join("\n");
        }
    }
    // Then without the volatile prompt line (echo + output stay fresh).
    if base.len() > 1 {
        let core = &base[..base.len() - 1];
        for j in 0..core.len() {
            if new.len() >= core.len() - j && new[..core.len() - j] == core[j..] {
                return new[core.len() - j..].join("\n");
            }
        }
        // Scrolled-off fallback: cut after the newest surviving
        // baseline line — except an end-anchored prompt line, which
        // would swallow genuine output (it matches the new prompt).
        for (bi, b) in base.iter().enumerate().rev() {
            if b.trim().is_empty() {
                continue;
            }
            if let Some(pos) = new.iter().rposition(|l| l == b) {
                if pos + 1 == new.len() && bi == base.len() - 1 {
                    continue;
                }
                return new[pos + 1..].join("\n");
            }
        }
    }
    out.trim().to_string()
}

/// Follow-up budget after the first settle window: ~20 rounds × ~15s
/// ≈ 5 minutes. Never-ending runs (servers, watchers) stop here with a
/// single terminal tail + `/read` pointer; `/read` covers the rest.
/// Nothing posts before that — no provisional cards, no footers: one
/// command yields one result card (a fresh tab also gets one ⏳ receipt
/// naming the new pane — routing, not a result). A posted result card
/// means done, except the rare budget-exhaust pointer (still running,
/// terminal, says so).
/// Typing is the liveness signal while it runs (per-settle sustain
/// covers forum topics and DMs alike).
pub(crate) const SHELL_FOLLOW_UP_ROUNDS: u32 = 20;

/// Pure result-card body so tests cover the shape without I/O: bare
/// `$ cmd` + tail, never a footer. Deliberately footerless — with no
/// provisional cards, a posted card is always the terminal report, so
/// footers would only restate what presence already says.
pub fn shell_result_text(cmd: &str, out: &str) -> String {
    let lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    format_shell_reply(cmd, &tail_fit(&lines, 3500))
}

/// True when the pane now holds an agent (shell→agent flip mid-settle,
/// e.g. `opencode` typed at the prompt): the shell report retires
/// silently instead of posting tails over a live agent session — the
/// icon flip + agent status are the whole signal. Read-only (`?` and
/// errors read as not-flipped: fail-closed, the loop just continues).
async fn flipped_to_agent(s: &AppState, pane: &str) -> bool {
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => a.kind != "?" && a.kind != "shell",
        Err(_) => false,
    }
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

/// One shell submit's settle inputs (bundles the 7 settle args — the
/// 8th, the submit generation, would trip clippy's 7-arg cap alone).
pub(crate) struct ShellSettle {
    pub chat: i64,
    pub thread: Option<i64>,
    pub pane: String,
    pub cmd: String,
    pub before: String,
    pub kb: Option<Value>,
    pub epoch: u64,
}

/// Settle one shell command and report it with exactly one card: fast
/// runs post their tail at once; long runs poll silently (typing is the
/// liveness signal) until they finish, then post the single tail — never
/// provisional cards, never footers. A shell→agent flip retires silently
/// (the icon flip is the signal). Delivery-tracked and cancel-aware: a
/// failed send keeps the intent for boot-recover, a cleared intent
/// (/cancel) stays silent.
pub(crate) async fn settle_report_shell(s: &AppState, st: ShellSettle) {
    let ShellSettle {
        chat,
        thread,
        pane,
        cmd,
        before,
        kb,
        epoch,
    } = st;
    let (pane, cmd, before) = (pane.as_str(), cmd.as_str(), before.as_str());
    // 1:1 working↔typing: shells run up to ~5 min with no watcher.
    // `start_typing` no-ops in DM; the instant touch below plus the
    // settle loop's sustain task cover both (spawned, never awaited —
    // a slow send must not delay the first settle read).
    s.start_typing(pane).await;
    {
        let tg = s.tg.clone();
        tokio::spawn(async move {
            tg.typing(chat, thread).await;
        });
    }
    let (out, settled) = await_shell_settle(s, pane, before, chat, thread).await;
    // /cancel during the settle clears the intent: a stale card must not
    // post for cancelled work. Generation-guarded too: submit order is
    // remember-then-bump, so check pending first and epoch last — any
    // resubmit that remembered necessarily bumped after, and the epoch
    // re-check catches even a byte-identical re-command.
    if !s.pending_matches(pane, chat, thread, cmd).await || !s.shell_epoch_is(pane, epoch).await {
        s.stop_shell_typing(pane).await;
        return;
    }
    // Shell→agent flip (`opencode` at the prompt): the shell intent is
    // now an agent session — retire silently, no cards at all.
    if flipped_to_agent(s, pane).await {
        s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
            .await;
        s.stop_shell_typing(pane).await;
        return;
    }
    // Fresh-only: `out` is the whole scrollback — delta against the
    // pre-send screen so one command's card never carries old output
    // (the follow-up below already deltas against the sent screen).
    // Unsettled posts NOTHING (typing = liveness): provisional cards
    // are noise — the single completion card is the whole report.
    if settled {
        let fresh = shell_result_text(cmd, &fresh_since(&out, before));
        let mid = s.tg.send_msg(chat, thread, &fresh, kb.clone()).await;
        s.remember(chat, mid, pane).await;
        if mid.is_none() {
            // Delivery-tracked: the intent survives for boot-recover
            // instead of eating the reply (typing stops too: nothing
            // sustains one now — boot-recover re-arms both on restart).
            s.stop_shell_typing(pane).await;
            return;
        }
        // Match-guarded: a resubmit racing the send above owns the slot now.
        s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
            .await;
        s.stop_shell_typing(pane).await;
        return;
    }
    let sent_lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    let mut settle_base = out;
    for _ in 0..SHELL_FOLLOW_UP_ROUNDS {
        let (next, done) = await_shell_settle(s, pane, &settle_base, chat, thread).await;
        settle_base = next.clone();
        if !s.pending_matches(pane, chat, thread, cmd).await || !s.shell_epoch_is(pane, epoch).await
        {
            s.stop_shell_typing(pane).await;
            return;
        }
        // Late flip (agent took over mid-run): same silent retire.
        if flipped_to_agent(s, pane).await {
            s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
                .await;
            s.stop_shell_typing(pane).await;
            return;
        }
        if !done {
            continue; // silent poll: no partial-card spam
        }
        let new_lines: Vec<String> = next.lines().map(|l| l.trim_end().to_string()).collect();
        let fresh = delta(&new_lines, &sent_lines).to_vec();
        let body = if fresh.iter().all(|l| l.trim().is_empty()) {
            format!("$ {cmd}\n(no further output)")
        } else {
            shell_result_text(cmd, &fresh.join("\n"))
        };
        let mid2 = s.tg.send_msg(chat, thread, &body, kb.clone()).await;
        s.remember(chat, mid2, pane).await;
        if mid2.is_some() {
            s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
                .await;
        }
        s.stop_shell_typing(pane).await;
        return;
    }
    // Budget exhausted and still a shell: the single terminal tail (or
    // a short note when nothing new arrived all budget) + `/read`
    // pointer. Terminal, not transient — nothing more posts for this
    // command — and rare (agent flips retire above long before this).
    // Flip-guarded like the loop: the last round may have passed the
    // check as shell just before the flip landed.
    if !s.pending_matches(pane, chat, thread, cmd).await || !s.shell_epoch_is(pane, epoch).await {
        s.stop_shell_typing(pane).await;
        return;
    }
    if flipped_to_agent(s, pane).await {
        s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
            .await;
        s.stop_shell_typing(pane).await;
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
        format!(
            "{}\n⏳ still running — output above may grow; `/read` for more.",
            shell_result_text(cmd, &fresh.join("\n"))
        )
    };
    let mid3 = s.tg.send_msg(chat, thread, &body, kb).await;
    s.remember(chat, mid3, pane).await;
    if mid3.is_some() {
        s.clear_shell_if_matches(pane, chat, thread, cmd, epoch)
            .await;
    }
    s.stop_shell_typing(pane).await;
}

#[cfg(test)]
#[path = "shell_common_tests.rs"]
mod tests;
