//! Restore/remint regression tests (split from `tests`, 300-line file limit).
use super::TopicStorage;

#[test]
fn test_flat_prev_restores_when_main_missing() {
    // Legacy flat `.prev` must restore like a full one: a flat map
    // parses as an empty Store (unknown fields ignored), so returning
    // it directly mass-remints while last-good sits next to it.
    let path =
        std::env::temp_dir().join(format!("herdr-tg-test-flatprev-{}.json", super::test_tag()));
    let prev = std::path::PathBuf::from(format!("{}.prev", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
    std::fs::write(&prev, r#"{"w1:p1": 42}"#).unwrap();
    let re = TopicStorage::at(path.clone());
    assert_eq!(re.get_thread("w1:p1"), Some(42));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&prev);
}

#[test]
fn test_remint_clears_last_msgs_and_rejects_general_thread() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-remint-{}.json", super::test_tag())),
    );
    st.insert_with_title("w1:p1".into(), 42, "t");
    st.record_msg("w1:p1", 100);
    assert!(!st.get_recent_msgs("w1:p1").is_empty());
    // Remint drops the dead topic's mids (remove_if_thread parity).
    st.insert_with_title("w1:p1".into(), 43, "t2");
    assert!(st.get_recent_msgs("w1:p1").is_empty());
    // Same-thread re-title keeps mids.
    st.record_msg("w1:p1", 101);
    st.insert_with_title("w1:p1".into(), 43, "t3");
    assert!(!st.get_recent_msgs("w1:p1").is_empty());
    // Thread 1 (General) is never a pane topic.
    st.insert_with_title("w1:p9".into(), 1, "general");
    assert_eq!(st.get_thread("w1:p9"), None);
    assert_eq!(st.get_pane(1), None);
    let _ = std::fs::remove_file(&st.file_path);
}
