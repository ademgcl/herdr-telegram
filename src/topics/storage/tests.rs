//! Storage round-trip tests. Split from `storage` (300-line file limit).
use super::{Store, TopicStorage};

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
    assert_eq!(st.get_thread("w1:p1"), Some(42));
    assert_eq!(st.get_title("w1:p1"), Some("api".to_string()));
    assert_eq!(
        st.get_icon("w1:p1"),
        Some("5350554349074391003".to_string())
    );
    st.clear_all();
    assert_eq!(st.get_thread("w1:p1"), None);
    assert_eq!(st.get_title("w1:p1"), None);
    assert_eq!(st.get_icon("w1:p1"), None);
    let _ = std::fs::remove_file(&st.file_path);
}
