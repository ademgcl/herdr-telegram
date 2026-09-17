use crate::{
    herdr::client::{get_agent, read_screen_visible},
    jobs::{filter::deframe, segment::dialog_block, stream::join_trimmed},
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

mod options;
#[cfg(test)]
mod tests;

pub use options::{has_numbered_options, parse_options};

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
    // Same source as taps (live_card Rule 5): options off the raw
    // winner lines — cleaned text drops ❯-rows and would fall back
    // to Confirm while the tap path shows real options.
    let (q, options) = live_card(screen);
    let mid =
        s.tg.send_msg_with_effect(
            chat,
            thread,
            &blocked_card_text(&q),
            Some(blocked_kb(pane, &options)),
            Some(crate::telegram::EFFECT_FIRE),
        )
        .await;
    let posted = mid.is_some();
    // Stamp the signature only on delivery: a dropped card must stay
    // "new" so the next observation reposts instead of going silent.
    if let Some(m) = mid {
        s.blocked_sig.lock().await.insert(pane.to_string(), q);
        let _ = s.tg.set_reaction(chat, m, Some("❗")).await;
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
    // Taps own the update: a tap in flight wins over an observation.
    // Refresh-refresh single-flight happens at send time below (claim +
    // sig re-check), so a slow watchdog read never rejects user taps.
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
    // Send-time single-flight: claim, then re-check the sig — a racing
    // refresh/tap that posted while we were reading wins, we stand down.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        return false;
    };
    if s.blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == &sig)
        .unwrap_or(false)
    {
        return false;
    }
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
