//! Tests for [`super::shell_settle`] (split: 300-line file limit).
use super::*;

// Three identical idle reads settle; the first two only bank.
#[test]
fn test_settle_poll_idle_settles_on_third_identical() {
    let (n, done) = settle_poll("echo", "before", "", 0, Some(true));
    assert_eq!((n, done), (0, false));
    let (n, done) = settle_poll("echo", "before", "echo", 0, Some(true));
    assert_eq!((n, done), (1, false));
    let (n, done) = settle_poll("echo", "before", "echo", 1, Some(true));
    assert_eq!((n, done), (2, true));
}

#[test]
fn test_settle_poll_busy_never_vests() {
    // The echo-stable trap: frozen screen, command still running —
    // the streak resets every round, no matter how long it sits.
    for _ in 0..10 {
        let (n, done) = settle_poll("echo", "before", "echo", 0, Some(false));
        assert_eq!((n, done), (0, false));
    }
    // Even a banked streak dies the moment busy is observed.
    let (n, done) = settle_poll("echo", "before", "echo", 1, Some(false));
    assert_eq!((n, done), (0, false));
}

#[test]
fn test_settle_poll_unchanged_or_moved_banks_nothing() {
    // Screen identical to pre-send: input not yet rendered.
    assert_eq!(settle_poll("same", "same", "same", 0, Some(true)), (0, false));
    // Screen still moving: output streaming in.
    assert_eq!(
        settle_poll("new", "before", "old", 1, Some(true)),
        (0, false)
    );
}

#[test]
fn test_settle_poll_unknown_busy_needs_longer_bar() {
    // Timing-only fallback end to end: the entry read banks nothing,
    // so five identical reads settle, never three.
    let mut last = String::new();
    let mut n = 0;
    for round in 0..5 {
        let (next, done) = settle_poll("echo", "before", &last, n, None);
        n = next;
        last = "echo".to_string();
        assert_eq!(done, round == 4);
    }
    assert_eq!(n, 4);
    // Busy still wins over banked unknown streaks.
    let (n, done) = settle_poll("echo", "before", "echo", 3, Some(false));
    assert_eq!((n, done), (0, false));
}

#[test]
fn test_note_blank_banks_nothing_and_dies_on_full_window() {
    // Blanks increment only their own streak: isolated blanks never
    // settle, a window of nothing but blanks settles dead.
    let (n, dead) = note_blank(0);
    assert_eq!((n, dead), (1, false));
    let (n, dead) = note_blank(13);
    assert_eq!((n, dead), (14, false));
    let (n, dead) = note_blank(14);
    assert_eq!((n, dead), (15, true));
}
