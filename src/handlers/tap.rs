/// Button taps and typed answers for blocked dialogs (see dialog.rs for
/// card building/posting). Split out to respect the 300-line file cap.
use std::time::Duration;
use serde_json::Value;
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, dialog_sig, parse_options, refresh_blocked_card, waiting_lines},
    handlers::interactive::{keys_for, opt_keys},
    herdr::client::{get_agent, read_screen_visible, send_agent_keys, send_pane_input},
    state::AppState,
};

/// Post-tap screen: a new dialog, a resumed agent, or an unchanged one.
#[derive(Debug, PartialEq)]
pub enum TapResult {
    NewDialog,
    Resumed,
    Unchanged,
}

/// Pure classification so taps never mislabel a turned-over dialog as
/// "resumed" (the old bug: option sets differed, so `closed` wrongly
/// held) and never strip buttons off a dialog that is still up. Ground
/// truth is fresh herdr status, not screen tea-leaves: `still_blocked`
/// decides resumed; the screens only distinguish a new dialog from an
/// unchanged one. Empty `after` (unreadable screen) never touches the card.
pub fn classify_tap(before: &[String], after: &[String], still_blocked: bool) -> TapResult {
    if !still_blocked {
        return TapResult::Resumed;
    }
    if after.is_empty() {
        return TapResult::Unchanged;
    }
    if parse_options(after) != parse_options(before) {
        return TapResult::NewDialog;
    }
    TapResult::Unchanged
}

struct TapSend {
    nav: Vec<&'static str>,
    confirm: Vec<&'static str>,
    label: String,
}

enum TapCall {
    Unknown,
    KeysFailed,
    Landed(TapSend, Vec<String>, Vec<String>, bool),
}

/// Send the tap's keys and re-read. Returns what to classify — never
/// touches Telegram itself (the caller owns the card).
async fn tap_keys(socket: &str, pane: &str, action: &str) -> TapCall {
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
            _ => return TapCall::Unknown,
        }
    } else {
        match keys_for(action) {
            Some(keys) => (Vec::new(), keys.to_vec(), action.to_string()),
            None => return TapCall::Unknown,
        }
    };
    let before = read_screen_visible(socket, pane, 30).await;
    if !nav.is_empty() {
        if send_agent_keys(socket, pane, &nav).await.is_err() {
            return TapCall::KeysFailed;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    if send_agent_keys(socket, pane, &confirm).await.is_err() {
        return TapCall::KeysFailed;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = read_screen_visible(socket, pane, 30).await;
    // Ground truth for "resumed": fresh status beats screen heuristics
    // (a working screen full of prose is not a dialog, and a lagging
    // status flip is covered by the observation layer's sig check).
    let still_blocked = get_agent(socket, pane).await.map(|a| a.status == "blocked").unwrap_or(true);
    TapCall::Landed(TapSend { nav, confirm, label }, before, after, still_blocked)
}

/// Button-tap entry point: sends keys, then brings the TAPPED card up
/// to date in place — a turned-over dialog swaps question + buttons, a
/// resumed agent gets its buttons stripped (stale taps must never inject
/// keys into live work). Feedback rides the card edit; only ambiguous
/// outcomes send a message.
pub async fn answer_tap(s: &AppState, chat: i64, msg_id: i64, thread: Option<i64>, pane: &str, action: &str) {
    if action == "type" {
        s.typewait.lock().await.insert(chat, pane.to_string());
        let mid = s.tg.send_msg(chat, thread, "⌨️ type your answer as the next message (⏎ sends it)", None).await;
        s.remember(chat, mid, pane).await;
        return;
    }
    s.blockop.lock().await.insert(pane.to_string());
    let call = tap_keys(&s.cfg.socket, pane, action).await;
    s.blockop.lock().await.remove(pane);
    match call {
        TapCall::Unknown => {
            let mid = s.tg.send_msg(chat, thread, "unknown button", None).await;
            s.remember(chat, mid, pane).await;
        }
        TapCall::KeysFailed => {
            let mid = s.tg.send_msg(chat, thread, "⚠️ keys failed — answer on the PC", None).await;
            s.remember(chat, mid, pane).await;
        }
        TapCall::Landed(send, before, after, still_blocked) => match classify_tap(&before, &after, still_blocked) {
            TapResult::NewDialog => {
                let q = waiting_lines(&after);
                let opts = parse_options(&after);
                s.tg.edit_msg(chat, msg_id, &blocked_card_text(&q), Some(blocked_kb(pane, &opts))).await;
                s.blocked_sig.lock().await.insert(pane.to_string(), q);
                s.remember(chat, Some(msg_id), pane).await;
            }
            TapResult::Resumed => {
                let no_kb = Some(Value::Array(Vec::new()));
                s.tg.edit_msg(chat, msg_id, &format!("✅ {} answered — agent resumed [{pane}]", send.label), no_kb).await;
                s.remember(chat, Some(msg_id), pane).await;
            }
            TapResult::Unchanged => {
                let mut shown = send.nav.clone();
                shown.extend(send.confirm.iter().cloned());
                let sent = shown.join("+");
                let before_opts = parse_options(&before);
                let text = if before_opts.is_empty() {
                    format!("⌨️ sent {sent} — check the pane")
                } else {
                    format!("⚠️ sent {sent} but the dialog still shows — highlight may have moved; try Esc or answer on the PC")
                };
                let mid = s.tg.send_msg(chat, thread, &text, None).await;
                s.remember(chat, mid, pane).await;
            }
        },
    }
}

/// Type free text into the waiting prompt (y/n answers, picker filters,
/// text inputs) + Enter, atomically: split text/Enter round-trips get
/// lost on redraw-heavy TUIs. Verified like button taps — a lying
/// "typed" ack is worse than none. The answer may advance to a SECOND
/// dialog with no status change, so re-check shortly and surface fresh
/// buttons.
pub async fn type_text(s: &AppState, pane: &str, text: &str) -> Result<(), String> {
    let socket = &s.cfg.socket;
    let before = read_screen_visible(socket, pane, 30).await;
    send_pane_input(socket, pane, text).await.map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    if get_agent(socket, pane).await.map(|a| a.status == "blocked").unwrap_or(true) {
        let after = read_screen_visible(socket, pane, 30).await;
        if dialog_stalled(&before, &after) {
            return Err("text sent but the dialog didn't advance — tap a button instead, or answer on the PC".into());
        }
    }
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        refresh_blocked_card(&s2, &pane2).await;
    });
    Ok(())
}

/// True when a typed answer provably went nowhere: both screens readable
/// and the dialog identical. Unreadable screens never fail the send.
pub fn dialog_stalled(before: &[String], after: &[String]) -> bool {
    !before.is_empty() && !after.is_empty() && dialog_sig(after) == dialog_sig(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_classify_new_dialog_on_turnover() {
        // allow → confirm: option sets differ while still blocked → new
        // card, never "resumed".
        let before = v(&["△ Permission required", "Allow once   Allow always   Reject"]);
        let after = v(&["Confirm apply?", "Confirm   Cancel"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
    }

    #[test]
    fn test_classify_new_dialog_when_card_had_none() {
        let before = v(&["△ Permission required"]);
        let after = v(&["Pick a model", "Sonnet   Opus   Haiku"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
    }

    #[test]
    fn test_classify_resumed_when_no_longer_blocked() {
        // Ground truth is status: whatever the screen shows, a moved-on
        // agent means resumed.
        let before = v(&["△ Permission required", "Allow once   Allow always   Reject"]);
        let after = v(&["⠋ working…", "editing src/main.rs"]);
        assert_eq!(classify_tap(&before, &after, false), TapResult::Resumed);
    }

    #[test]
    fn test_classify_unchanged_same_dialog() {
        let dlg = v(&["△ Permission required", "Allow once   Allow always   Reject"]);
        assert_eq!(classify_tap(&dlg, &dlg, true), TapResult::Unchanged);
        // Unreadable screen never touches the card, either way.
        assert_eq!(classify_tap(&v(&["x"]), &[], true), TapResult::Unchanged);
        assert_eq!(classify_tap(&v(&["x"]), &[], false), TapResult::Resumed);
    }

    #[test]
    fn test_dialog_stalled_same_dialog() {
        let dlg = v(&["△ Permission required", "Allow once   Allow always   Reject"]);
        assert!(dialog_stalled(&dlg, &dlg));
        // Typed text echoing into the field counts as progress.
        let filled = v(&["△ Permission required", "Allow once   Allow always   Reject", "my reason"]);
        assert!(!dialog_stalled(&dlg, &filled));
        // Unreadable screens never fail the send.
        assert!(!dialog_stalled(&[], &dlg));
        assert!(!dialog_stalled(&dlg, &[]));
    }
}
