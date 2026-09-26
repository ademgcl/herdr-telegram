use crate::{jobs::persist::PendingPrompt, state::AppState, ui::tail_fit};

// Re-exported for the submit paths (`shell_run`, boot-recover): the
// snapshot reader lives in `shell_settle` with its poll-loop consumer;
// the settle+report loop lives in `shell_report` (300-line split).
pub(crate) use super::shell_report::{ShellSettle, settle_report_shell};
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

/// Pure result-card body so tests cover the shape without I/O: bare
/// `$ cmd` + tail, never a footer. Deliberately footerless — with no
/// provisional cards, a posted card is always the terminal report, so
/// footers would only restate what presence already says.
pub fn shell_result_text(cmd: &str, out: &str) -> String {
    let lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    format_shell_reply(cmd, &tail_fit(&lines, 3500))
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

/// Fresh classify inputs (single source): owed + live-job + was_shell
/// read together, immediately before classify — an early was_shell
/// observation goes stale vs owed/job across reconcile's side-effect
/// awaits (a status flip landing mid-await skipped RetireVanished).
/// Status is volatile (empty at boot): unknown consults the durable
/// shell marker (creation tag/icon) before crying flip.
pub(crate) async fn shell_reuse_inputs(
    s: &AppState,
    pane: &str,
) -> (Option<PendingPrompt>, bool, bool) {
    let owed = s.pending.lock().await.get(pane).cloned();
    let has_job = s.job_live(pane).await;
    let was_shell = match s.status.lock().await.get(pane).cloned() {
        Some(st) => st == "shell",
        None => s.topics.is_shell_tagged(pane),
    };
    (owed, has_job, was_shell)
}

#[cfg(test)]
#[path = "shell_common_tests.rs"]
mod tests;
