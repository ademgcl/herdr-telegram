use super::*;
use crate::state::live::LiveSlot;
use std::time::Duration;

fn acc(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn slot(mid: i64, turn: u64) -> LiveSlot {
    LiveSlot {
        chat: 7,
        thread: None,
        mid,
        text: "t".to_string(),
        at: None,
        turn,
    }
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
async fn test_keep_mode_retire_leaves_slot_for_reuse() {
    // Auto-remove off (the default): the retire is a no-op — same
    // message, same slot, next turn reuses it with zero new
    // notification entries. No RPC fires here either.
    let (s, _dir) = crate::state::cancel::isolated_state();
    assert!(!s.transient_remove());
    s.live_put("w1:p1", slot(42, 3)).await;
    clear_live_if_epoch(&s, "w1:p1", Some((42, 3))).await;
    clear_live(&s, "w1:p1").await;
    let kept = s.live_get("w1:p1").await.expect("slot kept");
    assert_eq!((kept.mid, kept.turn), (42, 3));
}

#[tokio::test]
async fn test_moved_on_generation_stands_retire_down() {
    // Entry mismatch returns before any RPC (safe with auto-remove on:
    // no delete fires for a slot the retire does not own).
    let (s, _dir) = crate::state::cancel::isolated_state();
    s.set_transient_remove(true).await;
    s.live_put("w1:p1", slot(42, 4)).await;
    // No entry at all: nothing to retire.
    clear_live_if_epoch(&s, "w1:p1", None).await;
    // Stale entry (old mid / old gen): successor owns it now.
    clear_live_if_epoch(&s, "w1:p1", Some((41, 4))).await;
    clear_live_if_epoch(&s, "w1:p1", Some((42, 3))).await;
    let kept = s.live_get("w1:p1").await.expect("slot kept");
    assert_eq!((kept.mid, kept.turn), (42, 4));
    s.set_transient_remove(false).await;
}
