//! Blocked-pane dialogs (permission confirms, pickers, y/n prompts).
//! herdr rejects `agent.prompt` on blocked panes, so answers go through
//! keys/buttons/typed text. Two staleness traps live here:
//! * consecutive dialogs with NO status change (allow → confirm): cards
//!   follow dialog *content* ([`refresh_blocked_card`] is
//!   content-addressed via [`dialog_sig`]), never transitions alone;
//! * taps landing mid-turnover: [`answer_tap`] swaps the tapped card in
//!   place, strips dead buttons on resume, and never leaves first-dialog
//!   buttons armed over a second dialog.
use crate::{
    herdr::client::read_screen_visible,
    jobs::{filter::deframe, segment::dialog_block, stream::join_trimmed},
    state::AppState,
};
use serde_json::{Value, json};

mod options;
mod refresh;
mod surfaces;
#[cfg(test)]
mod tests;

pub use options::{has_numbered_options, parse_options};
pub use refresh::refresh_blocked_card;
pub(crate) use surfaces::{resolve_cards, settle_card, strip_tracked};

/// Question + options off one screen, parsed from the same final-block
/// source the cards use — tap paths must never diverge from it.
pub(crate) fn live_card(screen: &[String]) -> (String, Vec<String>) {
    let lines: Vec<String> = screen.iter().map(|l| deframe(l)).collect();
    let (cleaned, raw) = dialog_block(&lines);
    let body = join_trimmed(&cleaned);
    let q = if body.is_empty() {
        "(waiting for input)".to_string()
    } else {
        body
    };
    // Rule 5: options parsed from raw winner lines so terminal padding
    // never eats the options row before parse_options sees it.
    let mut opts = parse_options(&raw);
    if opts.is_empty() {
        opts = parse_options(&q.lines().map(str::to_string).collect::<Vec<_>>());
    }
    (q, opts)
}

/// Question lines off the VISIBLE screen (the only source herdr serves
/// on blocked panes), de-framed so dialog text and option rows survive.
pub(crate) fn waiting_lines(screen: &[String]) -> String {
    let lines: Vec<String> = screen.iter().map(|l| deframe(l)).collect();
    let (cleaned, _) = dialog_block(&lines);
    let body = join_trimmed(&cleaned);
    if body.is_empty() {
        "(waiting for input)".to_string()
    } else {
        body
    }
}

/// Signature of the displayed dialog: same dialog → same sig across
/// re-reads; a turned-over dialog (allow → confirm, or same question
/// with different options) → new sig. Options included: an
/// option-only turnover must not reuse the sig and go silent with
/// stale buttons.
pub(crate) fn dialog_sig(screen: &[String]) -> String {
    let q = waiting_lines(screen);
    let opts = parse_options(&winner_lines(screen));
    if opts.is_empty() {
        q
    } else {
        format!("{}\n{}", q, opts.join("\n"))
    }
}

/// Winner-segment lines for shape checks (option parse, text-input
/// gate): a stale numbered list in scrollback must not flip bounds or
/// the text gate. Falls back to the full screen when no block wins —
/// callers stay permissive on unreadable screens, like `tap_keys`.
/// Pure for tests.
pub(crate) fn winner_lines(screen: &[String]) -> Vec<String> {
    if screen.is_empty() {
        return Vec::new();
    }
    let lines: Vec<String> = screen.iter().map(|l| deframe(l)).collect();
    let win = dialog_block(&lines).1;
    if win.is_empty() { screen.to_vec() } else { win }
}

fn short_label(opt: &str, idx: usize) -> String {
    let label: String = opt.chars().take(12).collect();
    format!("{}: {label}", idx + 1)
}

/// Action goes second: pane ids contain ':' (wG:p1), so `splitn(3, ':')`
/// must keep the pane whole — `B:allow:wG:p1`, never `B:wG:p1:allow`.
/// Option taps encode their index: `B:opt2:wG:p1`.
pub(crate) fn blocked_kb(pane: &str, options: &[String]) -> Value {
    let mut rows = Vec::new();
    let mut opt_row = Vec::new();
    for (i, opt) in options.iter().take(8).enumerate() {
        opt_row.push(json!({
            "text": short_label(opt, i),
            "callback_data": format!("B:opt{i}:{pane}"),
        }));
        if opt_row.len() == 4 {
            rows.push(Value::Array(std::mem::take(&mut opt_row)));
        }
    }
    if !opt_row.is_empty() {
        rows.push(Value::Array(opt_row));
    }
    if rows.is_empty() {
        rows.push(json!([
            {"text": "✅ Confirm ⏎", "callback_data": format!("B:allow:{pane}")},
        ]));
    }
    // 1:1 with the live TUI: an option dialog is answered by navigating
    // its options — free text has nowhere to land (typed keys + Enter
    // can confirm the wrong highlight). Type shows only for option-less
    // dialogs (text inputs), where it is the only way to answer.
    let mut dismiss_row =
        vec![json!({"text": "🚫 Dismiss", "callback_data": format!("B:deny:{pane}")})];
    if options.is_empty() {
        dismiss_row
            .push(json!({"text": "⌨️ Type answer", "callback_data": format!("B:type:{pane}")}));
    }
    rows.push(Value::Array(dismiss_row));
    Value::Array(rows)
}

pub(crate) fn blocked_card_text(q: &str, options: &[String]) -> String {
    // Same 1:1: promise typing only where the button exists.
    let hint = if options.is_empty() {
        "\n\nTap an answer, or just type it."
    } else {
        "\n\nTap an answer."
    };
    format!("⛔ blocked — needs input\n\n{q}{hint}")
}

async fn send_with(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    screen: &[String],
) -> bool {
    // Same source as taps (live_card Rule 5): options off the raw
    // winner lines — cleaned text drops ❯-rows and would fall back
    // to Confirm while the tap path shows real options.
    let (q, options) = live_card(screen);
    let mid =
        s.tg.send_msg_with_effect(
            chat,
            thread,
            &blocked_card_text(&q, &options),
            Some(blocked_kb(pane, &options)),
            Some(crate::telegram::EFFECT_FIRE),
        )
        .await;
    let posted = mid.is_some();
    // Stamp the signature only on delivery: a dropped card must stay
    // "new" so the next observation reposts instead of going silent.
    // Track the location too: a PC-side answer strips it (buttons must
    // not outlive the dialog they answered).
    if let Some(m) = mid {
        // Stamp the full dialog_sig (question + options, never bare q):
        // refresh compares sigs, so a bare-q stamp never matches an
        // option dialog and reposts every tick (spam).
        s.blocked_sig
            .lock()
            .await
            .insert(pane.to_string(), dialog_sig(screen));
        surfaces::repoint_card(s, pane, chat, m).await;
        let _ = s.tg.set_reaction(chat, m, Some("❗")).await;
    }
    s.remember(chat, mid, pane).await;
    posted
}

/// True when the screen carries no dialog content at all: blank
/// question AND no options. A whitespace-only/outage read must not
/// buzz `(waiting for input)` + ❗ — but a blank question WITH options
/// is a real picker and still posts. Pure for tests.
pub(crate) fn is_blank_card(screen: &[String]) -> bool {
    let (q, opts) = live_card(screen);
    opts.is_empty() && q == "(waiting for input)"
}

/// Post the interactive card: what the agent asks + one-tap answers
/// mirroring the dialog's own options. Explicit "show me" contexts use
/// this (always posts); status observations use [`refresh_blocked_card`].
/// Returns true when the card was delivered. Empty/unreadable screens
/// post nothing (parity with refresh — an explicit `/card` must not
/// buzz `(waiting for input)` + ❗ on outage).
pub async fn send_blocked_card(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) -> bool {
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    if screen.is_empty() || is_blank_card(&screen) {
        return false;
    }
    send_with(s, chat, thread, pane, &screen).await
}

/// Retire a pane's dialog generation (topic remint/reset): the old
/// topic is gone, so drop the content sig (else refresh stays silent
/// on a stale match in the new, card-less topic) and its dead card
/// locations. The next observation reposts fresh via the live mapping.
pub async fn retire_dialog(s: &AppState, pane: &str) {
    s.blocked_sig.lock().await.remove(pane);
    s.blocked_card.lock().await.remove(pane);
}
