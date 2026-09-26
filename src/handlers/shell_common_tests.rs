//! Tests for [`super::shell_common`] (split: 300-line file limit).
use super::*;

#[test]
fn test_format_shell_reply() {
    assert_eq!(
        format_shell_reply("pwd", "/home/user/projects").as_str(),
        "$ pwd\n/home/user/projects"
    );
    assert_eq!(
        format_shell_reply("true", "  \n ").as_str(),
        "$ true\n(no output)"
    );
}

#[test]
fn test_shell_card_text() {
    let t = shell_card_text("w1:p1");
    assert!(t.contains("w1:p1") && t.contains("re-enter"));
}

#[test]
fn test_shell_result_text_notes() {
    // One command, one bare tail card — no footers, no transient lines.
    let out = "line1\nline2";
    let card = shell_result_text("make build", out);
    assert!(card.contains("$ make build"));
    assert!(card.contains("line2"));
    assert!(!card.contains("⏳") && !card.contains("✅") && !card.contains("following up"));
    // Empty output never posts a bare prompt line.
    assert!(shell_result_text("true", "  \n ").contains("(no output)"));
}

#[test]
fn test_classify_shell_reuse() {
    use ShellReuse::*;
    // Live shell work: never touch (the long-run intent-eat bug).
    assert_eq!(classify_shell_reuse(true, true, false), Ignore);
    // Stale watcher on a live shell: job only, intent preserved.
    assert_eq!(classify_shell_reuse(true, true, true), CancelJob);
    assert_eq!(classify_shell_reuse(true, false, true), CancelJob);
    // Fresh agent→shell flip: full retire + quit notice.
    assert_eq!(classify_shell_reuse(false, true, false), RetireVanished);
    assert_eq!(classify_shell_reuse(false, false, true), RetireVanished);
    assert_eq!(classify_shell_reuse(false, true, true), RetireVanished);
    // Nothing owed: ignore.
    assert_eq!(classify_shell_reuse(false, false, false), Ignore);
    assert_eq!(classify_shell_reuse(true, false, false), Ignore);
}

#[tokio::test]
async fn test_shell_reuse_inputs_read_fresh_at_classify() {
    // Was-shell reorder regression: the triple must be read together at
    // classify time — a status flip landing after earlier observations
    // (pre-fix: was_shell read before the side-effect awaits) must show
    // up in the same tuple as owed/job, or RetireVanished gets skipped.
    use crate::jobs::job::Job;
    use crate::state::cancel::isolated_state;
    let (s, _dir) = isolated_state();
    s.status
        .lock()
        .await
        .insert("t:p1".into(), "working".into());
    let (owed, has_job, was) = shell_reuse_inputs(&s, "t:p1").await;
    assert!(owed.is_none() && !has_job && !was);
    assert_eq!(
        classify_shell_reuse(was, owed.is_some(), has_job),
        ShellReuse::Ignore
    );
    // Flip + successor work land: the next read sees ALL three fresh.
    s.status.lock().await.insert("t:p1".into(), "shell".into());
    s.jobs
        .lock()
        .await
        .insert("t:p1".into(), Job::new(vec![], 1, None));
    s.pending.lock().await.insert(
        "t:p1".into(),
        crate::jobs::persist::PendingPrompt {
            chat: 1,
            thread: None,
            prompt: "hi".into(),
            started_unix: 1,
        },
    );
    let (owed, has_job, was) = shell_reuse_inputs(&s, "t:p1").await;
    assert!(
        owed.is_some() && has_job && was,
        "inputs must read fresh together"
    );
    assert_eq!(
        classify_shell_reuse(was, owed.is_some(), has_job),
        ShellReuse::CancelJob
    );
    // Volatile status empty → durable shell-tag fallback, never a
    // hardcoded false (boot would cry flip on every shell pane).
    s.status.lock().await.remove("t:p1");
    let (_, _, was) = shell_reuse_inputs(&s, "t:p1").await;
    assert_eq!(was, s.topics.is_shell_tagged("t:p1"));
}

#[test]
fn test_fresh_since_strips_scrollback() {
    // Old output never reposts: only lines after the baseline survive.
    let before = "$ ls\nold1\nold2\n$ ";
    let out = "$ ls\nold1\nold2\n$ ls\nnew1\nnew2\n$ ";
    let fresh = fresh_since(out, before);
    assert!(!fresh.contains("old1"));
    assert!(fresh.contains("new1") && fresh.contains("new2"));
}

#[test]
fn test_fresh_since_empty_when_unchanged() {
    // No new output (quiet command) → empty, rendering as (no output).
    let screen = "$ true\n$ ";
    assert_eq!(fresh_since(screen, screen), "");
}

#[test]
fn test_fresh_since_shallow_window() {
    // Shallow snapshots delta the same way: a scrolled window still
    // opens with a baseline suffix — commands never bleed.
    let before = "b1\nb2";
    let out = "b2\nc1";
    assert_eq!(fresh_since(out, before), "c1");
}

#[test]
fn test_fresh_since_end_prompt_never_swallows_output() {
    // The baseline prompt reappears as the new prompt: cutting after it
    // would return "" and eat genuine output — degrade instead.
    let before = "old1\nold2\n$ ";
    let out = "$ cmd\ngenuine\n$ ";
    let fresh = fresh_since(out, before);
    assert!(fresh.contains("genuine"));
    assert!(!fresh.contains("old1"));
}

#[test]
fn test_fresh_since_scroll_plus_prompt_replace() {
    // Top scrolled off AND prompt replaced by the echo: both halves align.
    let before = "l0\nl1\nl2\n$ ";
    let out = "l2\n$ cmd\nres\n$ ";
    let fresh = fresh_since(out, before);
    assert!(fresh.contains("res"));
    assert!(!fresh.contains("l0") && !fresh.contains("l1") && !fresh.contains("l2"));
}
