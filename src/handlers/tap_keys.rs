use crate::{
    handlers::interactive::{keys_for, opt_keys},
    herdr::client::{get_agent, read_screen_visible, send_agent_keys},
};
use std::time::Duration;

pub(crate) struct TapSend {
    pub(crate) nav: Vec<&'static str>,
    pub(crate) confirm: Vec<&'static str>,
    pub(crate) label: String,
}

pub(crate) enum TapCall {
    Unknown,
    KeysFailed,
    Landed(TapSend, Vec<String>, Vec<String>, bool),
}

/// Numbered-dialog key for option tap index `i`: cards emit at most 8
/// options (dialog takes 8) — wider indices are crafted callbacks,
/// never real buttons. Pure single source for the `i < 8` gate below
/// and its regression test.
pub(crate) fn opt_tap_num(i: usize) -> Option<&'static str> {
    match i {
        0 => Some("1"),
        1 => Some("2"),
        2 => Some("3"),
        3 => Some("4"),
        4 => Some("5"),
        5 => Some("6"),
        6 => Some("7"),
        7 => Some("8"),
        _ => None,
    }
}

/// Option-index bounds gate (pure, tested): the card index against the
/// live probe — shared by the pre-send gate above and the post-nav
/// revalidation below so the two can never drift (a stale index into a
/// narrower turned-over dialog stays silent, never drives keys blind).
fn opt_bounds_hold(i: usize, probe: &[String]) -> bool {
    opt_tap_num(i).is_some() && i < crate::handlers::dialog::parse_options(probe).len()
}

/// Dialog-presence gate (pure, tested): named keys need a live dialog
/// shape (parsed options or a question) — shared by the pre-send gate
/// and the post-nav revalidation like the bounds gate above.
fn dialog_present(probe: &[String]) -> bool {
    !crate::handlers::dialog::parse_options(probe).is_empty()
        || crate::handlers::dialog::dialog_sig(probe).contains('?')
}

/// Post-nav revalidation (async, RPC): the nav sleep below is a turnover
/// window — re-apply the pre-send gates (blocked status + per-action
/// bounds/presence) on a fresh read before confirming, or Enter lands
/// on the NEW dialog's highlight (wrong-option injection into
/// turned-over work). Fail-closed on unreadable reads like the gates.
async fn confirm_still_valid(socket: &str, pane: &str, action: &str) -> bool {
    let screen =
        read_screen_visible(socket, pane, crate::handlers::dialog::DIALOG_READ_LINES).await;
    if screen.is_empty() {
        return false;
    }
    match get_agent(socket, pane).await {
        Ok(a) if a.status != "blocked" => return false,
        Err(_) => return false,
        _ => {}
    }
    let win = crate::handlers::dialog::winner_lines(&screen);
    let probe: &[String] = if win.is_empty() { &screen } else { &win };
    if let Some(rest) = action.strip_prefix("opt") {
        match rest.parse::<usize>() {
            Ok(i) => opt_bounds_hold(i, probe),
            Err(_) => false,
        }
    } else {
        dialog_present(probe)
    }
}

/// Send the tap's keys and re-read. Returns what to classify — never
/// touches Telegram itself (the caller owns the card).
pub(crate) async fn tap_keys(socket: &str, pane: &str, action: &str) -> TapCall {
    // Read first: the index is validated against the LIVE dialog below —
    // a stale card (or crafted callback) must never drive keys into a
    // narrower turned-over dialog. Unreadable screens fail closed (an
    // outage bricks taps until the blip passes — sending blind risks
    // wrong-option injection into a turned-over dialog); unreadable
    // status fails closed at the gate below, like `type_text`.
    let before =
        read_screen_visible(socket, pane, crate::handlers::dialog::DIALOG_READ_LINES).await;
    if before.is_empty() {
        return TapCall::Unknown;
    }
    // Stale-tap ground truth: buttons only exist on blocked panes. A tap
    // racing a resume — or landing days later on a history card — must
    // refuse instead of driving keys into live work (shape heuristics
    // alone cannot tell working prose from a dialog). Unreadable status
    // fails closed like `type_text` (an outage bricks taps until the
    // blip passes; sends would fail visibly below anyway).
    match get_agent(socket, pane).await {
        Ok(a) if a.status != "blocked" => return TapCall::Unknown,
        // Classified death (quit-to-shell race, closed pane) is a stale
        // tap — "already moved on", never "keys failed". Any other read
        // error is an outage/blip: sending blind risks wrong-option
        // injection, so fail closed like the screen gate above.
        Err(e) if crate::herdr::rpc::is_not_found(&e.to_string()) => return TapCall::Unknown,
        Err(_) => return TapCall::KeysFailed,
        _ => {}
    }
    // Winner-segment basis (single source: `winner_lines`): a stale
    // numbered list in scrollback must not flip the branch or the
    // bounds check.
    let win = crate::handlers::dialog::winner_lines(&before);
    let probe: &[String] = if win.is_empty() { &before } else { &win };
    let (nav, confirm, label): (Vec<&str>, Vec<&str>, String) =
        if let Some(rest) = action.strip_prefix("opt") {
            match rest.parse::<usize>() {
                // opt_keys is the single source of truth; Enter goes separately
                // so the TUI gets a render beat after navigating (else Enter
                // confirms the stale first highlight — the old Tab bug).
                // Cards emit at most 8 options (dialog takes 8): wider
                // indices are crafted callbacks, never real buttons.
                Ok(i) => {
                    // Bounds against the live dialog (opt_bounds_hold —
                    // same gate as the post-nav revalidation below): a
                    // stale index into a narrower turned-over dialog
                    // stays silent, and an option-less live screen
                    // refuses (never drive keys blind into
                    // turned-over work).
                    if !opt_bounds_hold(i, probe) {
                        return TapCall::Unknown;
                    }
                    let Some(num) = opt_tap_num(i) else {
                        return TapCall::Unknown;
                    };
                    let (full, confirm) = if crate::handlers::dialog::has_numbered_options(probe) {
                        (vec![num], vec!["enter"])
                    } else {
                        let mut full = opt_keys(i);
                        let Some(confirm) = full.pop() else {
                            return TapCall::Unknown;
                        };
                        (full, vec![confirm])
                    };
                    (full, confirm, format!("option {}", i + 1))
                }
                _ => return TapCall::Unknown,
            }
        } else {
            match keys_for(action) {
                Some(keys) => {
                    // Never inject keys blind: without a live dialog shape
                    // (parsed options or a question) on screen, Enter / Esc
                    // / Right would land in live work. (B:type arming never
                    // reaches here — it sends no keys.)
                    if !dialog_present(probe) {
                        return TapCall::Unknown;
                    }
                    (Vec::new(), keys.to_vec(), action.to_string())
                }
                None => return TapCall::Unknown,
            }
        };
    if !nav.is_empty() {
        if let Err(e) = send_agent_keys(socket, pane, &nav).await {
            // Confirmed death (quit-to-shell race) is a stale tap —
            // "already moved on", never "keys failed".
            if crate::herdr::rpc::is_not_found(&e.to_string()) {
                return TapCall::Unknown;
            }
            return TapCall::KeysFailed;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
        // Turnover window: a new dialog landing in the sleep owns the
        // highlight now — confirming would inject the wrong option
        // into turned-over work. Re-gate on a fresh read (fail-closed);
        // a stale tap strips + heals downstream like the pre-send gate.
        if !confirm_still_valid(socket, pane, action).await {
            return TapCall::Unknown;
        }
    }
    if let Err(e) = send_agent_keys(socket, pane, &confirm).await {
        if crate::herdr::rpc::is_not_found(&e.to_string()) {
            return TapCall::Unknown;
        }
        return TapCall::KeysFailed;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = read_screen_visible(socket, pane, crate::handlers::dialog::DIALOG_READ_LINES).await;
    // Ground truth for "resumed": fresh status beats screen heuristics
    // (a working screen full of prose is not a dialog, and a lagging
    // status flip is covered by the observation layer's sig check).
    // Fail-closed like the pre-send gate above and `type_text`'s
    // post-send gate: an unreadable status must not read as "still
    // blocked" (a ghost NewDialog card with live buttons into live
    // work when the tap actually resumed). The KeysFailed arm
    // converges (strip + heal) and the retry re-validates live.
    let still_blocked = match get_agent(socket, pane).await {
        Ok(a) => a.status == "blocked",
        // Pre-send-gate parity: confirmed death is a stale tap ("already
        // moved on" + strip), only a blip reads as keys-failed + heal.
        Err(e) if crate::herdr::rpc::is_not_found(&e.to_string()) => return TapCall::Unknown,
        Err(_) => return TapCall::KeysFailed,
    };
    TapCall::Landed(
        TapSend {
            nav,
            confirm,
            label,
        },
        before,
        after,
        still_blocked,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opt_tap_num_covers_8_and_rejects_wider() {
        // Cards emit at most 8 options: 0–7 map to TUI keys 1–8,
        // anything wider is a crafted callback, never a real button.
        assert_eq!(
            (0..8).map(opt_tap_num).collect::<Vec<_>>(),
            vec![
                Some("1"),
                Some("2"),
                Some("3"),
                Some("4"),
                Some("5"),
                Some("6"),
                Some("7"),
                Some("8")
            ]
        );
        assert_eq!(opt_tap_num(8), None);
        assert_eq!(opt_tap_num(99), None);
    }

    fn probe(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_opt_bounds_hold_live_and_turned_over() {
        // Pre-send and post-nav gates share this: a live 3-option dialog
        // admits 0–2; a turned-over narrower dialog (or option-less
        // screen) refuses — Enter must never confirm a fresh highlight.
        let live = probe(&["Allow once   Allow always   Reject"]);
        assert!(opt_bounds_hold(0, &live));
        assert!(opt_bounds_hold(2, &live));
        assert!(!opt_bounds_hold(3, &live));
        // Crafted wide index refused even against a live dialog.
        assert!(!opt_bounds_hold(9, &live));
        let narrow = probe(&["Allow once   Deny"]);
        assert!(opt_bounds_hold(0, &narrow));
        assert!(!opt_bounds_hold(2, &narrow));
        assert!(!opt_bounds_hold(0, &probe(&["steady working prose"])));
    }

    #[test]
    fn test_dialog_present_options_or_question() {
        // Named keys need a dialog shape: options or a question.
        assert!(dialog_present(&probe(&[
            "Allow once   Allow always   Reject"
        ])));
        // Bare working prose (no options, no question) refuses.
        assert!(!dialog_present(&probe(&["steady working prose"])));
        assert!(!dialog_present(&probe(&[])));
    }
}
