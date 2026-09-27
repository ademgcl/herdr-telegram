//! Tag assignment tests (split from `tests`: 500-line file limit).
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

/// A tag stored before the pane number became the identity must MIGRATE,
/// or that pane keeps a legacy `o4` in its title forever while its
/// neighbours show the real number — which is exactly the mixed state a
/// live forum ends up in.
#[test]
fn test_legacy_code_tag_migrates_to_the_pane_number() {
    let st = TopicStorage::at(
        std::env::temp_dir().join(format!("herdr-tg-test-migrate-{}.json", super::test_tag())),
    );
    // Seed the old shape directly: code-prefixed tags from before.
    let file = st.file_path.clone();
    let raw = std::fs::read_to_string(&file).unwrap_or_default();
    let mut v: serde_json::Value =
        serde_json::from_str(if raw.is_empty() { "{}" } else { &raw }).unwrap();
    v["tags"]["wT:p2"] = serde_json::json!("o2");
    v["tags"]["wT:p1"] = serde_json::json!("pi1");
    v["tags"]["wV:p6"] = serde_json::json!("sh3");
    std::fs::write(&file, v.to_string()).expect("seed");
    let st = TopicStorage::at(file.clone());

    assert_eq!(st.assign_tag("wT:p2", "opencode"), "2");
    assert_eq!(st.assign_tag("wT:p1", "pi"), "1");
    assert_eq!(
        st.assign_tag("wV:p6", "shell"),
        "6",
        "shell loses its sh prefix too"
    );
    // Idempotent: a second call changes nothing.
    assert_eq!(st.assign_tag("wT:p2", "opencode"), "2");
    let _ = std::fs::remove_file(&file);
}
