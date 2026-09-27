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
fn test_streamed_tail_renders_bare_model_text_chrome_stripped() {
    // The transient shows the model's actual text: TUI furniture
    // (tool echoes, progress verbs, status bars) strips, prose stays,
    // and no "working" header poses as agent output.
    let body = render_progress(&acc(&[
        "✱ Read src/main.rs",
        "Thinking…",
        "hello",
        " ⬝⬝⬝⬝⬝⬝⬝⬝ esc interrupt   145.6K (14%)  ctrl+p commands    ~/projects/herdr-telegram:main",
    ]));
    assert_eq!(body, "hello");
}

#[test]
fn test_chrome_only_tail_falls_back_to_thinking() {
    // Tool echoes alone are not a reply: placeholder until prose streams.
    assert_eq!(
        render_progress(&acc(&["✱ Read src/main.rs", "⠸ Writing command…"])),
        THINKING
    );
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
    assert!(body.encode_utf16().count() <= crate::types::MAX_MSG_UNITS);
}

#[test]
fn test_not_modified_reads_as_converged() {
    // Telegram rejects a no-op edit; treating it as a failure logged a
    // bogus error on every turn.
    assert!(not_modified("Bad Request: message is not modified"));
    assert!(not_modified("Bad Request: MESSAGE_NOT_MODIFIED"));
    assert!(!not_modified("Bad Request: message to edit not found"));
    assert!(!not_modified("Too Many Requests: retry after 5"));
}

#[test]
fn test_retry_at_banks_flood_window_then_falls_back() {
    let now = Instant::now();
    // Flood: bank the full window plus retry_after's own +1s safety
    // margin (retrying inside `retry after N` only extends the ban).
    assert_eq!(
        retry_at(&now, "Too Many Requests: retry after 7") - now,
        Duration::from_secs(8)
    );
    // Anything else: bounded backoff, never one retry per tick.
    assert_eq!(
        retry_at(&now, "connection reset by peer") - now,
        Duration::from_secs(EDIT_RETRY_BACKOFF_SECS)
    );
}

#[test]
fn test_edit_due_same_text_never() {
    let now = Instant::now();
    assert!(!edit_due("a", "a", None, now));
    assert!(!edit_due("a", "a", Some(now), now));
}

#[test]
fn test_edit_due_first_edit_always() {
    // First edit of real content always goes through (no prior attempt).
    assert!(edit_due("", "real content", None, Instant::now()));
    // But editing TO the placeholder is never allowed (would overwrite
    // delivered output with "thinking…").
    assert!(!edit_due(
        "delivered answer",
        THINKING,
        None,
        Instant::now()
    ));
    assert!(!edit_due(THINKING, THINKING, None, Instant::now()));
}

#[test]
fn test_edit_due_future_attempt_never_panics_and_throttles() {
    // Flood-waits bank a FUTURE attempt time: must read as throttled.
    let now = Instant::now();
    assert!(!edit_due(
        "a",
        "b",
        Some(now + Duration::from_secs(60)),
        now
    ));
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
async fn test_keep_mode_retire_leaves_slot_for_the_next_turn() {
    // Auto-remove off (the default): the retire is a no-op — the slot
    // survives so the next turn can continue the generation lineage and
    // post its OWN message beside it (it never edits this one). No RPC
    // fires here either.
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

/// The transient must show the marked answer, not the tool transcript
/// streaming behind it.
#[test]
fn test_transient_renders_the_marked_reply_not_the_tool_dump() {
    use crate::jobs::reply_block::{REPLY_CLOSE, REPLY_OPEN};
    let acc = acc_of(&[
        "   ┃  $ cargo test 2>&1 | tail -12",
        "   ┃  test result: ok. 549 passed; 0 failed",
        REPLY_OPEN,
        "Working on it — the parser is the real cause.",
        REPLY_CLOSE,
    ]);
    let body = super::render_progress(&acc);
    assert!(
        body.contains("the parser is the real cause"),
        "transient showed something else: {body:?}"
    );
    for junk in ["cargo test", "549 passed", "[["] {
        assert!(
            !body.contains(junk),
            "tool output in the transient: {junk:?} in {body:?}"
        );
    }
}

/// A previous turn's markers must not pose as this turn's status.
#[test]
fn test_transient_ignores_previous_turns_markers() {
    use crate::jobs::reply_block::{REPLY_CLOSE, REPLY_OPEN};
    let acc = acc_of(&[
        "  ┃  older message",
        REPLY_OPEN,
        "stale answer from the previous turn",
        REPLY_CLOSE,
        "  ┃  newer message",
        "  ┃  $ cargo test",
        REPLY_OPEN,
        "fresh answer for this turn only",
        REPLY_CLOSE,
    ]);
    let body = super::render_progress_for(&acc, "newer message");
    assert!(
        body.contains("fresh answer for this turn only"),
        "transient showed something else: {body:?}"
    );
    assert!(
        !body.contains("stale answer from the previous turn"),
        "re-served the previous turn: {body:?}"
    );
}

fn acc_of(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}
