//! Storage round-trip tests. Split from `storage` (300-line file limit).
use super::{TopicStorage, disk::Store};

#[test]
fn test_store_roundtrip_and_migration() {
    // Legacy flat map migrates through the real read path; current
    // format (incl. unknown future fields) reads as-is.
    let leg = std::env::temp_dir().join(format!("herdr-tg-test-legacy-{}.json", super::test_tag()));
    std::fs::write(&leg, r#"{"w1:p1": 42}"#).unwrap();
    let st = TopicStorage::at(leg.clone());
    assert_eq!(st.get_thread("w1:p1"), Some(42));
    let _ = std::fs::remove_file(&leg);

    let cur = std::env::temp_dir().join(format!("herdr-tg-test-cur-{}.json", super::test_tag()));
    std::fs::write(&cur, r#"{"topics": {"w2:p2": 7}, "future_field": true}"#).unwrap();
    let st2 = TopicStorage::at(cur.clone());
    assert_eq!(st2.get_thread("w2:p2"), Some(7));
    let _ = std::fs::remove_file(&cur);

    let s = Store::default();
    assert!(s.topics.is_empty());
    // Removed `unread` set still loads (unknown-field tolerant): a legacy
    // file carrying it must not fail the read path.
    let leg2 =
        std::env::temp_dir().join(format!("herdr-tg-test-unread-{}.json", super::test_tag()));
    std::fs::write(&leg2, r#"{"topics": {"w3:p3": 9}, "unread": ["w3:p3"]}"#).unwrap();
    let st3 = TopicStorage::at(leg2.clone());
    assert_eq!(st3.get_thread("w3:p3"), Some(9));
    let _ = std::fs::remove_file(&leg2);
}

#[test]
fn test_persistence_across_reopen() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-topics-{}.json", super::test_tag())),
    );
    st.insert("w9:p9".into(), 77);
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.get_thread("w9:p9"), Some(77));
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_tags_stable_and_reassigned() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-tags-{}.json", super::test_tag())),
    );
    assert_eq!(st.assign_tag("w1:p1", "opencode"), "o1");
    assert_eq!(st.assign_tag("w1:p1", "opencode"), "o1");
    assert_eq!(st.assign_tag("w1:p2", "opencode"), "o2");
    assert_eq!(st.assign_tag("w2:p1", "claude"), "c1");
    // Persisted across reopen; freed tags are refilled.
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.assign_tag("w1:p2", "opencode"), "o2");
    re.remove("w1:p1");
    assert_eq!(re.assign_tag("w3:p9", "opencode"), "o1");
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_titles_roundtrip() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-titles-{}.json", super::test_tag())),
    );
    assert_eq!(st.get_title("w1:p1"), None);
    st.set_title("w1:p1", "api");
    st.set_title("w1:p1", "api"); // idempotent, no extra write
    assert_eq!(st.get_title("w1:p1"), Some("api".to_string()));
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.get_title("w1:p1"), Some("api".to_string()));
    re.remove("w1:p1");
    assert_eq!(re.get_title("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_clear_all() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-clear-{}.json", super::test_tag())),
    );
    st.insert("w1:p1".into(), 42);
    st.set_title("w1:p1", "api");
    st.set_icon("w1:p1", "5350554349074391003");
    st.set_pin("w1:p1", 999);
    assert_eq!(st.get_thread("w1:p1"), Some(42));
    assert_eq!(st.get_title("w1:p1"), Some("api".to_string()));
    assert_eq!(
        st.get_icon("w1:p1"),
        Some("5350554349074391003".to_string())
    );
    assert_eq!(st.get_pin("w1:p1"), Some(999));
    st.clear_all();
    assert_eq!(st.get_thread("w1:p1"), None);
    assert_eq!(st.get_title("w1:p1"), None);
    assert_eq!(st.get_icon("w1:p1"), None);
    assert_eq!(st.get_pin("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_pins_roundtrip() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-pins-{}.json", super::test_tag())),
    );
    assert_eq!(st.get_pin("w1:p1"), None);
    st.set_pin("w1:p1", 1234);
    st.set_pin("w1:p1", 1234); // idempotent
    assert_eq!(st.get_pin("w1:p1"), Some(1234));
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.get_pin("w1:p1"), Some(1234));
    re.remove("w1:p1");
    assert_eq!(re.get_pin("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_set_pin_if_thread_guards_remint() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-pincas-{}.json", super::test_tag())),
    );
    st.insert("w1:p1".into(), 42);
    assert!(st.set_pin_if_thread("w1:p1", 42, 111));
    assert_eq!(st.get_pin("w1:p1"), Some(111));
    // Stale thread (pre-remint send) must not clobber the fresh pin.
    st.insert("w1:p1".into(), 43);
    assert!(!st.set_pin_if_thread("w1:p1", 42, 222));
    assert_eq!(st.get_pin("w1:p1"), Some(111));
    assert!(st.set_pin_if_thread("w1:p1", 43, 333));
    assert_eq!(st.get_pin("w1:p1"), Some(333));
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_insert_with_title_clears_pin_on_remint() {
    let st = TopicStorage::at(std::env::temp_dir().join(format!(
        "herdr-tg-test-pinremint-{}.json",
        super::test_tag()
    )));
    st.insert_with_title("w1:p1".into(), 42, "a");
    st.set_pin("w1:p1", 111);
    // Same thread re-insert keeps the pin (no spurious repost).
    st.insert_with_title("w1:p1".into(), 42, "a");
    assert_eq!(st.get_pin("w1:p1"), Some(111));
    // Changed thread (remint) drops it — a failed fresh-card send must
    // not leave edits aimed at the deleted message.
    st.insert_with_title("w1:p1".into(), 43, "b");
    assert_eq!(st.get_pin("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_corrupt_state_falls_back_to_prev() {
    // A good save rotates a .prev backup; a later corrupt write loads
    // the backup instead of wiping every mapping (mass duplicates).
    let path = std::env::temp_dir().join(format!("herdr-tg-test-prev-{}.json", super::test_tag()));
    let prev = std::path::PathBuf::from(format!("{}.prev", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    let st = TopicStorage::at(path.clone());
    st.insert("w1:p1".into(), 42);
    assert!(prev.exists());
    std::fs::write(&path, "{corrupt").unwrap();
    let re = TopicStorage::at(path.clone());
    assert_eq!(re.get_thread("w1:p1"), Some(42));
    // Corrupt main AND corrupt backup → default (no crash, no hang).
    std::fs::write(&path, "{corrupt").unwrap();
    std::fs::write(&prev, "{corrupt").unwrap();
    let empty = TopicStorage::at(path.clone());
    assert_eq!(empty.get_thread("w1:p1"), None);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    for bak in std::fs::read_dir(std::env::temp_dir()).unwrap() {
        let bak = bak.unwrap().path();
        if bak.to_string_lossy().contains("herdr-tg-test-prev-")
            && bak.extension().is_some_and(|e| e == "bak")
        {
            let _ = std::fs::remove_file(&bak);
        }
    }
}

#[test]
fn test_recent_msgs_roundtrip_and_bounding() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-msgs-{}.json", super::test_tag())),
    );
    assert_eq!(st.get_recent_msgs("w1:p1"), Vec::<i64>::new());
    st.record_msg("w1:p1", 101);
    st.record_msg("w1:p1", 101); // deduplicate consecutive
    assert_eq!(st.get_recent_msgs("w1:p1"), vec![101]);
    st.record_msg("w1:p1", 102);
    st.record_msg("w1:p1", 103);
    assert_eq!(st.get_recent_msgs("w1:p1"), vec![101, 102, 103]);
    // Bounded to 3
    st.record_msg("w1:p1", 104);
    assert_eq!(st.get_recent_msgs("w1:p1"), vec![102, 103, 104]);

    // Persistence across reopen
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.get_recent_msgs("w1:p1"), vec![102, 103, 104]);

    re.remove("w1:p1");
    assert_eq!(re.get_recent_msgs("w1:p1"), Vec::<i64>::new());
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_remove_tag_if_threadless_rolls_back_failed_create() {
    // Failed creates must not leak threadless tags (tag gaps forever):
    // a tag with no thread rolls back, a threaded tag survives.
    let st = TopicStorage::at(std::env::temp_dir().join(format!(
        "herdr-tg-test-tagrollback-{}.json",
        super::test_tag()
    )));
    let tag = st.assign_tag("w1:p9", "opencode");
    assert_eq!(st.get_tag("w1:p9"), Some(tag));
    assert_eq!(st.get_thread("w1:p9"), None);
    st.remove_tag_if_threadless("w1:p9");
    assert_eq!(st.get_tag("w1:p9"), None);
    // Threaded tags are never rolled back.
    st.assign_tag("w1:p1", "opencode");
    st.insert("w1:p1".into(), 42);
    st.remove_tag_if_threadless("w1:p1");
    assert!(st.get_tag("w1:p1").is_some());
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_empty_main_falls_back_to_prev() {
    // A zeroed main (failed copy, truncate, disk-full artifact) must
    // restore `.prev` like corrupt does — never wipe every mapping
    // (mass duplicate re-mints) while last-good sits next to it.
    let path = std::env::temp_dir().join(format!("herdr-tg-test-empty-{}.json", super::test_tag()));
    let prev = std::path::PathBuf::from(format!("{}.prev", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    let st = TopicStorage::at(path.clone());
    st.insert("w1:p1".into(), 42);
    assert!(prev.exists());
    std::fs::write(&path, "").unwrap();
    let re = TopicStorage::at(path.clone());
    assert_eq!(re.get_thread("w1:p1"), Some(42));
    // Empty main with no usable prev → default, no crash.
    std::fs::write(&prev, "{corrupt").unwrap();
    std::fs::write(&path, "   \n").unwrap();
    let empty = TopicStorage::at(path.clone());
    assert_eq!(empty.get_thread("w1:p1"), None);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
}

#[test]
fn test_clear_msgs_and_icon_drop_stale() {
    // Reset migration takes old-topic mids atomically (a second take
    // copies nothing stale); icon clear drops the custom so the
    // watchdog heals the kind glyph instead of wedging.
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-clear-{}.json", super::test_tag())),
    );
    st.insert("w1:p1".into(), 42);
    st.record_msg("w1:p1", 100);
    st.set_icon("w1:p1", "1234567890");
    assert_eq!(st.take_recent_msgs("w1:p1"), vec![100]);
    assert!(st.get_recent_msgs("w1:p1").is_empty());
    assert!(st.take_recent_msgs("w1:p1").is_empty());
    st.clear_icon("w1:p1");
    assert_eq!(st.get_icon("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_empty_object_falls_back_to_prev_and_tag_reassigns_on_flip() {
    // `{}` truncates like empty: last-good wins, never a mass re-mint.
    let path =
        std::env::temp_dir().join(format!("herdr-tg-test-emptyobj-{}.json", super::test_tag()));
    let prev = std::path::PathBuf::from(format!("{}.prev", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    let st = TopicStorage::at(path.clone());
    st.insert("w1:p1".into(), 42);
    assert!(prev.exists());
    std::fs::write(&path, "{}").unwrap();
    let re = TopicStorage::at(path.clone());
    assert_eq!(re.get_thread("w1:p1"), Some(42));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    // Shell→agent pane reuse must not keep the `sh*` family tag.
    let st2 = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-tagflip-{}.json", super::test_tag())),
    );
    let t1 = st2.assign_tag("w1:p1", "shell");
    assert!(t1.starts_with("sh"));
    let t2 = st2.assign_tag("w1:p1", "opencode");
    assert!(!t2.starts_with("sh"));
    assert_eq!(st2.get_tag("w1:p1").as_deref(), Some(t2.as_str()));
    let _ = std::fs::remove_file(&st2.file_path);
}
