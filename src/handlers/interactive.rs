/// Answering agents that wait for INTERACTIVE input (pickers, permission
/// dialogs, y/n prompts). herdr rejects `agent.prompt` on blocked panes,
/// so answers go through `agent.send_keys` (buttons/keys) or
/// `pane.send_text` (typed text + enter). No watcher job is ever created
/// for blocked panes: there is nothing to stream, only something to press.
///
/// Option buttons mirror the dialog itself: labels are parsed out of the
/// question card, option N is answered with Right×N + Enter (fresh dialogs
/// highlight the first option), and every tap is VERIFIED on-screen —
/// blind taps on permission dialogs would be security decisions in the
/// dark, so the ack reports whether the dialog actually closed.

use serde_json::{json, Value};
use crate::{
    handlers::model_scan::split_columns,
    herdr::client::{read_screen_visible, send_agent_keys, send_pane_text},
    jobs::{filter::deframe, segment::final_block, stream::join_trimmed},
    state::AppState,
};

static ALLOW: &[&str] = &["enter"];
static DENY: &[&str] = &["esc"];
/// Verified live against a real opencode permission dialog: Tab is
/// completely ignored there (highlight never moves), Right advances one
/// option, Left goes back (and wraps at the ends), number keys do nothing.
static NEXT: &[&str] = &["right"];

/// Callback action → keys. Enter confirms the highlighted (default)
/// option, Esc dismisses/denies, Right moves to the next option.
pub fn keys_for(action: &str) -> Option<&'static [&'static str]> {
    match action {
        "allow" => Some(ALLOW),
        "deny" => Some(DENY),
        "next" => Some(NEXT),
        _ => None,
    }
}

/// Key sequence answering option N (0-based): Right over N times, Enter.
/// Fresh dialogs highlight the first option — see `press` verification.
/// (Was Tab×N: opencode ignores Tab, so every tap confirmed option 1.)
pub fn opt_keys(idx: usize) -> Vec<&'static str> {
    let mut keys = vec!["right"; idx];
    keys.push("enter");
    keys
}

/// Lines that can never be options: key-hint rows and chrome-ish labels.
const HINT_WORDS: &[&str] = &[
    "ctrl",
    "enter",
    "select",
    "fullscreen",
    "esc",
    "shortcut",
    "navigate",
    "confirm",
    "dismiss",
    "cancel",
    "back",
    "quit",
];

/// Parse selectable option labels out of a question card: the first line
/// holding 2–4 short phrases in columns ("Allow once   Allow always
/// Reject"). Pure — tested below.
pub fn parse_options(lines: &[String]) -> Vec<String> {
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let low = t.to_lowercase();
        if HINT_WORDS.iter().any(|w| low.contains(w)) {
            continue;
        }
        let parts = split_columns(t);
        let short = parts.len() >= 2
            && parts.len() <= 4
            && parts.iter().all(|p| {
                let len = p.chars().count();
                len >= 2 && len <= 24 && p.split_whitespace().count() <= 3
            });
        if short {
            return parts;
        }
    }
    Vec::new()
}

/// Question lines off the VISIBLE screen (the only source herdr serves
/// on blocked panes), de-framed so dialog text and option rows survive.
pub(crate) fn waiting_lines(screen: &[String]) -> String {
    let lines: Vec<String> = screen.iter().map(|l| deframe(l)).collect();
    let body = join_trimmed(&final_block(&lines, ""));
    if body.is_empty() {
        "(waiting for input)".to_string()
    } else {
        body
    }
}

fn short_label(opt: &str, idx: usize) -> String {
    let label: String = opt.chars().take(12).collect();
    format!("{}: {label}", idx + 1)
}

/// Action goes second: pane ids contain ':' (wG:p1), so `splitn(3, ':')`
/// must keep the pane whole — `B:allow:wG:p1`, never `B:wG:p1:allow`.
/// Option taps encode their index: `B:opt2:wG:p1`.
fn blocked_kb(pane: &str, options: &[String]) -> Value {
    let mut opt_row = Vec::new();
    for (i, opt) in options.iter().take(4).enumerate() {
        opt_row.push(json!({
            "text": short_label(opt, i),
            "callback_data": format!("B:opt{i}:{pane}"),
        }));
    }
    let mut rows = Vec::new();
    if !opt_row.is_empty() {
        rows.push(Value::Array(opt_row));
    } else {
        rows.push(json!([
            {"text": "✅ Confirm ⏎", "callback_data": format!("B:allow:{pane}")},
        ]));
    }
    rows.push(json!([
        {"text": "🚫 Dismiss", "callback_data": format!("B:deny:{pane}")},
        {"text": "⌨️ Type answer", "callback_data": format!("B:type:{pane}")},
    ]));
    Value::Array(rows)
}

/// Post the interactive card: what the agent asks + one-tap answers
/// mirroring the dialog's own options. Used everywhere a blocked pane
/// surfaces (message routing, prompt-RPC rejection, empty blocked
/// settles) so replying always works.
pub async fn send_blocked_card(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    let q = waiting_lines(&screen);
    let options = parse_options(&q.lines().map(str::to_string).collect::<Vec<_>>());
    let text = format!("⛔ blocked — needs input\n\n{q}\n\nTap an answer, or just type it.");
    let mid = s.tg.send_msg(chat, thread, &text, Some(blocked_kb(pane, &options))).await;
    s.remember(chat, mid, pane).await;
}

/// Tap handler for the blocked card buttons. Key taps are verified: after
/// sending, the screen is re-read and the ack reports whether the dialog
/// actually closed — a tap that silently selected the wrong option must
/// never look like success.
pub async fn press(s: &AppState, chat: i64, pane: &str, action: &str) -> String {
    if action == "type" {
        s.typewait.lock().await.insert(chat, pane.to_string());
        return "⌨️ type your answer as the next message (⏎ sends it)".to_string();
    }
    let (nav, confirm, label): (Vec<&str>, Vec<&str>, String) = if let Some(rest) = action.strip_prefix("opt") {
        match rest.parse::<usize>() {
            // opt_keys is the single source of truth; Enter goes separately
            // so the TUI gets a render beat after navigating (else Enter
            // confirms the stale first highlight — the old Tab bug).
            Ok(i) if i < 6 => {
                let mut full = opt_keys(i);
                let confirm = vec![full.pop().unwrap()];
                (full, confirm, format!("option {}", i + 1))
            }
            _ => return "unknown button".to_string(),
        }
    } else {
        match keys_for(action) {
            Some(keys) => (Vec::new(), keys.to_vec(), action.to_string()),
            None => return "unknown button".to_string(),
        }
    };
    let before = read_screen_visible(&s.cfg.socket, pane, 30).await;
    let before_opts = parse_options(&before);
    if !nav.is_empty() {
        if send_agent_keys(&s.cfg.socket, pane, &nav).await.is_err() {
            return "⚠️ keys failed — answer on the PC".to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    }
    if send_agent_keys(&s.cfg.socket, pane, &confirm).await.is_err() {
        return "⚠️ keys failed — answer on the PC".to_string();
    }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let after = read_screen_visible(&s.cfg.socket, pane, 30).await;
    let after_text = after.join("\n");
    let closed = !before_opts.is_empty()
        && before_opts.iter().all(|o| !after_text.contains(o.as_str()));
    if closed {
        format!("✅ {label} answered — agent resumed")
    } else {
        let mut shown = nav.clone();
        shown.extend(confirm.iter().cloned());
        let sent = shown.join("+");
        if before_opts.is_empty() {
            format!("⌨️ sent {sent} — check the pane")
        } else {
            format!("⚠️ sent {sent} but the dialog still shows — highlight may have moved; try Esc or answer on the PC")
        }
    }
}

/// Type free text into the waiting prompt (y/n answers, picker filters,
/// text inputs) + Enter.
pub async fn type_text(s: &AppState, pane: &str, text: &str) -> Result<(), String> {
    send_pane_text(&s.cfg.socket, pane, text)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_keys_for_actions() {
        assert_eq!(keys_for("allow"), Some(&["enter"][..]));
        assert_eq!(keys_for("deny"), Some(&["esc"][..]));
        assert_eq!(keys_for("next"), Some(&["right"][..]));
        assert_eq!(keys_for("bogus"), None);
    }

    #[test]
    fn test_opt_keys_sequences() {
        assert_eq!(opt_keys(0), vec!["enter"]);
        assert_eq!(opt_keys(1), vec!["right", "enter"]);
        assert_eq!(opt_keys(2), vec!["right", "right", "enter"]);
    }

    #[test]
    fn test_columns_split() {
        assert_eq!(split_columns("Allow once   Allow always   Reject").len(), 3);
        assert_eq!(split_columns("Yes\tNo"), vec!["Yes", "No"]);
        assert_eq!(split_columns("single phrase here"), vec!["single phrase here"]);
        assert_eq!(split_columns("  padded   columns  "), vec!["padded", "columns"]);
    }

    #[test]
    fn test_parse_options_permission_row() {
        let lines = v(&[
            "△ Permission required",
            "Patterns",
            "- /Users/adem/.config/opencode/*",
            "Allow once   Allow always   Reject",
            "ctrl+f fullscreen  ⇆ select  enter confirm",
        ]);
        assert_eq!(
            parse_options(&lines),
            vec!["Allow once", "Allow always", "Reject"]
        );
    }

    #[test]
    fn test_parse_options_skips_hints_and_prose() {
        // Hint rows, plain prose and tables are never options.
        assert!(parse_options(&v(&["ctrl+f fullscreen  enter confirm"])).is_empty());
        assert!(parse_options(&v(&["Hello! How can I help you today?"])).is_empty());
        assert!(parse_options(&v(&["| a | b |", "| c | d |"])).is_empty());
        assert!(parse_options(&v(&["first line", "second line"])).is_empty());
    }

    #[test]
    fn test_callback_data_survives_pane_colons() {
        // Pane ids contain ':' — the action must come second so splitn(3)
        // keeps "wG:p1" whole.
        let kb = blocked_kb("wG:p1", &v(&["Allow once"]));
        let data = kb[0][0]["callback_data"].as_str().unwrap();
        let route: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(route, vec!["B", "opt0", "wG:p1"]);
    }
}
