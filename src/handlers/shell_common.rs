use crate::{herdr::client::read_shell_output, state::AppState};
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
/// did not, never present partial output as final.
pub(crate) async fn await_shell_settle(s: &AppState, pane: &str, before: &str) -> (String, bool) {
    let mut last = String::new();
    let mut stable = 0u32;
    let mut cur = String::new();
    for _ in 0..15 {
        sleep(Duration::from_secs(1)).await;
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
}
