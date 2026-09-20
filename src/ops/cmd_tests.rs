//! One-shot ops command tests. Split from `cmd` (300-line file limit).
use super::*;

#[test]
fn test_read_tail_from_seeks_and_caps() {
    // Unique per test (nanos, not pid-only): a crashed first run leaves
    // the dir behind — a pid-only path would re-enter with stale content
    // and fail the second run instead of the first.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("herdr-tail-test-{}-{nanos}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("t.log");
    std::fs::write(&path, b"aaaa\nbbbb\ncccc\ndddd\n").unwrap();
    // Full window from 0.
    let (b, at) = read_tail_from(&path, 0, 20, 1024).unwrap();
    assert_eq!((at, &b[..]), (0, &b"aaaa\nbbbb\ncccc\ndddd\n"[..]));
    // Gap over cap jumps the cursor.
    let (b, at) = read_tail_from(&path, 0, 20, 6).unwrap();
    assert_eq!(at, 14);
    assert_eq!(&b[..], b"\ndddd\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_count_topics_parses_store_not_lines() {
    // Envelope with 2 topics + tags/titles/pins/icons/last_msgs keys:
    // line-counting `":` reports ~10+, the parse reports 2.
    let t = r#"{"topics":{"w1:p1":1,"w1:p2":2},"tags":{"w1:p1":"a"},"titles":{"w1:p1":"t"},"pins":{},"icons":{},"last_msgs":{}}"#;
    assert_eq!(count_topics_in_text(t), 2);
    // Legacy flat map counts panes.
    assert_eq!(count_topics_in_text(r#"{"w1:p1":1}"#), 1);
    assert_eq!(count_topics_in_text("not json"), 0);
}
