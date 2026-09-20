//! Stale-tap gate for callback cards: split from `callback` (300-line
//! file limit). Pure verdicts so tests pin them.
use crate::{state::AppState, types::STALE_SECS};
use serde_json::Value;

/// Stale exemption (pure, tested): dialog (B) taps re-validate against
/// the live pane at tap time, so a long-lived blocked card stays
/// tappable; model list re-renders are read-only. Every other tap —
/// including M:<idx> switches (a side effect) — keeps the birth-date
/// gate below.
pub(crate) fn tap_exempt(head: &str, rest: Option<&str>) -> bool {
    head == "B" || (head == "M" && rest.map(|r| r.starts_with("list:")).unwrap_or(false))
}

/// Birth-date staleness (pure, tested): taps older than STALE_SECS must
/// not re-execute (days-old spawn-confirm/kill taps). Single source for
/// the gate in `callback`.
pub(crate) fn tap_stale(date: u64, now: u64) -> bool {
    now.saturating_sub(date) > STALE_SECS
}

/// Answer the spinner without stalling the sequential pump:
/// answerCallback sleeps on flood-wait — awaiting it inline would stall
/// the whole batch into a drop cascade (stale-notice parity below).
pub(crate) fn answer_spinner(s: &AppState, cbq: &Value) {
    if let Some(id) = cbq["id"].as_str() {
        let tg = s.tg.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tg.answer_callback(&id).await;
        });
    }
}

/// Stale-tap notice (spawned, never awaited — pump parity above).
pub(crate) fn notify_stale_card(s: &AppState, chat: i64, thread: Option<i64>) {
    println!("[callback] dropping stale tap");
    let tg = s.tg.clone();
    tokio::spawn(async move {
        tg.send_msg(
            chat,
            thread,
            "⌛️ that card expired — pick it again from `/agents`",
            None,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tap_exempt_dialog_and_list_only() {
        // Long-lived blocked cards stay tappable (live re-validation).
        assert!(tap_exempt("B", Some("opt0:w1:p1")));
        // Read-only list re-render is exempt…
        assert!(tap_exempt("M", Some("list:w1:p1")));
        // …but an M:<idx> switch is a side effect: gated.
        assert!(!tap_exempt("M", Some("0:w1:p1")));
        assert!(!tap_exempt("M", None));
        // Destructive arms always gated.
        assert!(!tap_exempt("k", Some("agent:w1")));
        assert!(!tap_exempt("X", Some("kill:w1:p1")));
    }

    #[test]
    fn test_tap_stale_birth_date_gate() {
        let now = 1_800_000_000;
        assert!(!tap_stale(now - 10, now));
        assert!(!tap_stale(now - STALE_SECS, now));
        assert!(tap_stale(now - STALE_SECS - 1, now));
        // Clock skew (date in future) never reads as stale.
        assert!(!tap_stale(now + 60, now));
    }
}
