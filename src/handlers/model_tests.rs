//! Tests for [`super`] (split: 300-line file limit).
use super::super::model_parse::free_tap;
use super::*;

#[test]
fn test_search_filter_prefers_zen() {
    assert_eq!(search_filter("gpt"), "gpt zen");
    assert_eq!(search_filter("gpt openrouter"), "gpt openrouter");
    assert_eq!(search_filter("Ling Flash"), "ling flash zen");
}

#[test]
fn test_free_tap_reexport() {
    assert!(free_tap(0).is_some());
    assert!(free_tap(99).is_none());
}

#[test]
fn test_model_kb_shape() {
    let kb = model_kb("w1:p1");
    // 7 free models, 2 per row → 4 rows; pane survives splitn(3).
    assert_eq!(kb.as_array().unwrap().len(), 4);
    let data = kb[0][0]["callback_data"].as_str().unwrap();
    assert_eq!(
        data.splitn(3, ':').collect::<Vec<_>>(),
        vec!["M", "0", "w1:p1"]
    );
    // Button labels are the shorts — full names never fit.
    assert_eq!(kb[0][0]["text"].as_str().unwrap(), "Big Pickle");
    assert_eq!(kb[0][1]["text"].as_str().unwrap(), "Muse Spark 1.3");
    assert!(
        kb.as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r.as_array().unwrap())
            .all(|b| { b["text"].as_str().unwrap().chars().count() <= 20 })
    );
}

#[test]
fn test_non_opencode_card() {
    let t = model_card_text(Some("x"), "w1:p1", "claude");
    assert!(t.contains("opencode-only"));
}
