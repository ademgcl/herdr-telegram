use super::*;
use std::time::Duration;

fn acc(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_empty_acc_renders_thinking_placeholder() {
    assert_eq!(render_progress(&[]), THINKING);
}

#[test]
fn test_streamed_tail_renders_under_working_header_with_transients_kept() {
    // Transient TUI lines are the point of progress: raw tail, kept.
    let body = render_progress(&acc(&["✱ Read src/main.rs", "Thinking…", "hello"]));
    assert!(body.starts_with("💭 working…\n\n"), "got {body:?}");
    assert!(body.contains("✱ Read src/main.rs"));
    assert!(body.contains("Thinking…"));
    assert!(body.contains("hello"));
}

#[test]
fn test_blank_tail_falls_back_to_thinking() {
    assert_eq!(render_progress(&acc(&["", "   "])), THINKING);
}

#[test]
fn test_long_tail_stays_within_telegram_cap() {
    // 500 padded lines must still fit one Telegram message (tail-fit,
    // never a 400-error send).
    let lines: Vec<String> = (0..500)
        .map(|i| format!("line {i:04} with padding text to grow the body past the cap"))
        .collect();
    let body = render_progress(&lines);
    assert!(body.starts_with("💭 working…"), "got {body:?}");
    assert!(body.encode_utf16().count() <= crate::types::MAX_MSG_UNITS);
}

#[test]
fn test_edit_due_same_text_never() {
    let now = Instant::now();
    assert!(!edit_due("a", "a", None, now));
    assert!(!edit_due("a", "a", Some(now), now));
}

#[test]
fn test_edit_due_first_edit_always() {
    assert!(edit_due("", THINKING, None, Instant::now()));
}

#[test]
fn test_edit_due_throttles_then_releases() {
    let t0 = Instant::now();
    assert!(!edit_due("a", "b", Some(t0), t0));
    assert!(!edit_due(
        "a",
        "b",
        Some(t0),
        t0 + Duration::from_secs(EDIT_COOLDOWN_SECS - 1)
    ));
    assert!(edit_due(
        "a",
        "b",
        Some(t0),
        t0 + Duration::from_secs(EDIT_COOLDOWN_SECS)
    ));
}

#[tokio::test]
async fn test_adopt_moves_slot_without_traffic() {
    let from = Job::new(vec!["b".into()], 7, None);
    let to = Job::new(vec!["b".into()], 7, None);
    *from.live_msg.lock().await = Some((7, None, 42));
    *from.live_text.lock().await = "hi".to_string();
    let (s, _dir) = crate::state::cancel::isolated_state();
    adopt_live(&s, &from, &to).await;
    assert!(from.live_msg.lock().await.is_none());
    assert_eq!(*to.live_msg.lock().await, Some((7, None, 42)));
    assert_eq!(*to.live_text.lock().await, "hi");
}

#[tokio::test]
async fn test_adopt_same_arc_noop() {
    let j = Job::new(vec!["b".into()], 7, None);
    *j.live_msg.lock().await = Some((7, None, 42));
    let (s, _dir) = crate::state::cancel::isolated_state();
    adopt_live(&s, &j, &j).await;
    assert_eq!(*j.live_msg.lock().await, Some((7, None, 42)));
}
