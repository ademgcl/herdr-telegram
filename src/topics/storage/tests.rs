//! Storage round-trip tests. Split from `storage` (300-line file limit).
use super::{TopicStorage, disk::Store};

#[test]
fn test_store_roundtrip_and_migration() {
    // Legacy flat map migrates through the real read path; current
    // format (incl. unknown future fields) reads as-is.
    let leg =
        std::env::temp_dir().join(format!("herdr-tg-test-legacy-{}.json", std::process::id()));
    std::fs::write(&leg, r#"{"w1:p1": 42}"#).unwrap();
    let st = TopicStorage::at(leg.clone());
    assert_eq!(st.get_thread("w1:p1"), Some(42));
    let _ = std::fs::remove_file(&leg);

    let cur = std::env::temp_dir().join(format!("herdr-tg-test-cur-{}.json", std::process::id()));
    std::fs::write(&cur, r#"{"topics": {"w2:p2": 7}, "future_field": true}"#).unwrap();
    let st2 = TopicStorage::at(cur.clone());
    assert_eq!(st2.get_thread("w2:p2"), Some(7));
    let _ = std::fs::remove_file(&cur);

    let s = Store::default();
    assert!(!s.unread.contains("x"));
}

#[test]
fn test_persistence_across_reopen() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-topics-{}.json", std::process::id())),
    );
    st.insert("w9:p9".into(), 77);
    let re = TopicStorage::at(st.file_path.clone());
    assert_eq!(re.get_thread("w9:p9"), Some(77));
    let _ = std::fs::remove_file(&st.file_path);
}

#[test]
fn test_tags_stable_and_reassigned() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-tags-{}.json", std::process::id())),
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
        std::env::temp_dir().join(format!("herdr-tg-test-titles-{}.json", std::process::id())),
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
        std::env::temp_dir().join(format!("herdr-tg-test-clear-{}.json", std::process::id())),
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
        std::env::temp_dir().join(format!("herdr-tg-test-pins-{}.json", std::process::id())),
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
        std::env::temp_dir().join(format!("herdr-tg-test-pincas-{}.json", std::process::id())),
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
        std::process::id()
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
    let path = std::env::temp_dir().join(format!("herdr-tg-test-prev-{}.json", std::process::id()));
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
        std::env::temp_dir().join(format!("herdr-tg-test-msgs-{}.json", std::process::id())),
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
fn test_empty_main_falls_back_to_prev() {
    // A zeroed main (failed copy, truncate, disk-full artifact) must
    // restore `.prev` like corrupt does — never wipe every mapping
    // (mass duplicate re-mints) while last-good sits next to it.
    let path =
        std::env::temp_dir().join(format!("herdr-tg-test-empty-{}.json", std::process::id()));
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
