//! Tag assignment tests (split from `tests`: 300-line file limit).
//! Tags are per-space NUMBERS now — the kind lives in the topic icon, so
//! nothing about a tag depends on the agent kind.
use super::TopicStorage;

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
    // Tags are kind-independent numbers now (the icon carries the kind),
    // so a shell→agent flip keeps the same tag instead of being forced
    // out of a `sh*` family that no longer exists.
    let st2 = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-tagflip-{}.json", super::test_tag())),
    );
    let t1 = st2.assign_tag("w1:p1", "shell");
    assert!(
        t1.chars().all(|c| c.is_ascii_digit()),
        "bare number: {t1:?}"
    );
    let t2 = st2.assign_tag("w1:p1", "opencode");
    assert_eq!(t1, t2, "a kind flip must not churn the tag");
    assert_eq!(st2.get_tag("w1:p1").as_deref(), Some(t2.as_str()));
    let _ = std::fs::remove_file(&st2.file_path);
}
