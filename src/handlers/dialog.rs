use crate::{
    handlers::model_scan::split_columns,
    herdr::client::{get_agent, read_screen_visible},
    jobs::{filter::deframe, segment::final_block, stream::join_trimmed},
    state::AppState,
};
/// Blocked-pane dialogs (permission confirms, pickers, y/n prompts).
/// herdr rejects `agent.prompt` on blocked panes, so answers go through
/// keys/buttons/typed text. Two staleness traps live here:
/// * consecutive dialogs with NO status change (allow → confirm): cards
///   follow dialog *content* ([`refresh_blocked_card]` is
///   content-addressed via [`dialog_sig`]), never transitions alone;
/// * taps landing mid-turnover: [`answer_tap`] swaps the tapped card in
///   place, strips dead buttons on resume, and never leaves first-dialog
///   buttons armed over a second dialog.
use serde_json::{Value, json};

/// Lines that can never be options: key-hint rows and chrome-ish labels.
/// NOTE: "confirm"/"cancel" are deliberately absent — real second
/// dialogs read "Confirm   Cancel", and hint rows carrying those words
/// always also carry ctrl/enter/esc/select (filtered on those instead).
const HINT_WORDS: &[&str] = &[
    "ctrl",
    "enter",
    "select",
    "fullscreen",
    "esc",
    "shortcut",
    "navigate",
    "dismiss",
    "back",
    "quit",
];

/// Selectable option labels out of a question card: the first line
/// holding 2–4 short phrases in columns ("Allow once   Allow always
/// Reject"). De-frames first: tap paths parse the raw visible screen
/// (frame rails intact), so a framed row must parse exactly like the
/// de-framed card text. Wide terminals merge the option row with the
/// right-aligned hint row on ONE terminal line
/// ("Allow once   Allow always   Reject   ctrl+f fullscreen …"), so hint
/// columns are filtered per-column — never by discarding the whole line.
/// Pure — tested below.
pub fn parse_options(lines: &[String]) -> Vec<String> {
    for line in lines {
        let t = deframe(line);
        if t.is_empty() {
            continue;
        }
        let parts = split_columns(&t);
        // Drop key-hint columns (right-aligned hints share the option
        // row on wide terminals); the remaining real options must be 2–4
        // short phrases. Hint-only rows filter to empty and are skipped.
        let real: Vec<String> = parts
            .into_iter()
            .filter(|p| {
                let low = p.to_lowercase();
                !HINT_WORDS.iter().any(|w| low.contains(w))
            })
            .collect();
        let short = real.len() >= 2
            && real.len() <= 4
            && real.iter().all(|p| {
                let len = p.chars().count();
                (2..=24).contains(&len) && p.split_whitespace().count() <= 3
            });
        if short {
            return real;
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

/// Signature of the displayed dialog: same dialog → same sig across
/// re-reads; a turned-over dialog (allow → confirm) → new sig.
pub(crate) fn dialog_sig(screen: &[String]) -> String {
    waiting_lines(screen)
}

fn short_label(opt: &str, idx: usize) -> String {
    let label: String = opt.chars().take(12).collect();
    format!("{}: {label}", idx + 1)
}

/// Action goes second: pane ids contain ':' (wG:p1), so `splitn(3, ':')`
/// must keep the pane whole — `B:allow:wG:p1`, never `B:wG:p1:allow`.
/// Option taps encode their index: `B:opt2:wG:p1`.
pub(crate) fn blocked_kb(pane: &str, options: &[String]) -> Value {
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

pub(crate) fn blocked_card_text(q: &str) -> String {
    format!("⛔ blocked — needs input\n\n{q}\n\nTap an answer, or just type it.")
}

async fn send_with(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    screen: &[String],
) -> bool {
    let q = waiting_lines(screen);
    let options = parse_options(&q.lines().map(str::to_string).collect::<Vec<_>>());
    let mid =
        s.tg.send_msg(
            chat,
            thread,
            &blocked_card_text(&q),
            Some(blocked_kb(pane, &options)),
        )
        .await;
    let posted = mid.is_some();
    // Stamp the signature only on delivery: a dropped card must stay
    // "new" so the next observation reposts instead of going silent.
    if posted {
        s.blocked_sig.lock().await.insert(pane.to_string(), q);
    }
    s.remember(chat, mid, pane).await;
    posted
}

/// Post the interactive card: what the agent asks + one-tap answers
/// mirroring the dialog's own options. Explicit "show me" contexts use
/// this (always posts); status observations use [`refresh_blocked_card`].
/// Returns true when the card was delivered.
pub async fn send_blocked_card(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) -> bool {
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    send_with(s, chat, thread, pane, &screen).await
}

/// Content-addressed blocked post for status observations: repeats of
/// the same dialog stay silent, a NEW dialog posts even with no status
/// transition. Skips while a tap is in flight (it owns the update) and
/// when fresh herdr truth says the pane already moved on.
pub async fn refresh_blocked_card(s: &AppState, pane: &str) -> bool {
    if s.blockop.lock().await.contains(pane) {
        return false;
    }
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status != "blocked" => {
            s.blocked_sig.lock().await.remove(pane);
            return false;
        }
        // No agent here: shells never need blocked cards (a delayed
        // refresh after quit-to-shell would post a ghost). A read error
        // on a dead pane fails below on the empty screen anyway.
        Err(_) => {
            let shell = crate::herdr::client::list_panes(&s.cfg.socket)
                .await
                .map(|l| l.contains(&pane.to_string()))
                .unwrap_or(false);
            if shell {
                s.blocked_sig.lock().await.remove(pane);
                return false;
            }
        }
        _ => {}
    }
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    if screen.is_empty() {
        return false;
    }
    let sig = dialog_sig(&screen);
    if s.blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == &sig)
        .unwrap_or(false)
    {
        return false;
    }
    // Baseline follows delivery: a dropped card stays "new" (sig
    // unstamped on failure), so the next observation reposts from an
    // intact baseline instead of skewing the later idle delta.
    let posted = if let Some(forum) = s.cfg.forum {
        let thread = s.topics.all_mappings().get(pane).copied();
        send_with(s, forum, thread, pane, &screen).await
    } else {
        let mut posted = false;
        for id in &s.cfg.owners {
            if send_with(s, *id, None, pane, &screen).await {
                posted = true;
            }
        }
        posted
    };
    if posted {
        s.seen.lock().await.insert(pane.to_string(), screen);
        println!("[alert] blocked card {pane}");
    }
    posted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
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
        assert!(parse_options(&v(&["ctrl+f fullscreen  enter confirm"])).is_empty());
        assert!(parse_options(&v(&["Hello! How can I help you today?"])).is_empty());
        assert!(parse_options(&v(&["| a | b |", "| c | d |"])).is_empty());
        assert!(parse_options(&v(&["first line", "second line"])).is_empty());
    }

    #[test]
    fn test_parse_options_framed_row() {
        // Tap paths parse the raw visible screen (rails intact) — a
        // framed row must yield the same options as card text.
        let lines = v(&[
            "┃ △ Permission required",
            "┃   Allow once   Allow always   Reject",
        ]);
        assert_eq!(
            parse_options(&lines),
            vec!["Allow once", "Allow always", "Reject"]
        );
    }

    #[test]
    fn test_parse_options_merged_hint_row() {
        // Wide terminals merge options + right-aligned hints on ONE row
        // (live opencode 1.18 dialog) — the real options must survive.
        let lines = v(&[
            "  ┃   Allow once   Allow always   Reject                                                                                 ctrl+f fullscreen  ⇆ select  enter confirm",
        ]);
        assert_eq!(
            parse_options(&lines),
            vec!["Allow once", "Allow always", "Reject"]
        );
        // Two-option dialog with merged hints.
        let lines = v(&["Confirm   Cancel   ctrl+f fullscreen  enter confirm"]);
        assert_eq!(parse_options(&lines), vec!["Confirm", "Cancel"]);
    }

    #[test]
    fn test_callback_data_survives_pane_colons() {
        let kb = blocked_kb("wG:p1", &v(&["Allow once"]));
        let data = kb[0][0]["callback_data"].as_str().unwrap();
        let route: Vec<&str> = data.splitn(3, ':').collect();
        assert_eq!(route, vec!["B", "opt0", "wG:p1"]);
    }
}
