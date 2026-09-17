//! Tests for [`super::panes`] (split: 300-line file limit).
use super::*;
use crate::herdr::labels::parse_facts;

fn facts(json: &str) -> HashMap<String, PaneFacts> {
    parse_facts(&serde_json::from_str(json).unwrap())
}

#[test]
fn test_pick_first_pane_prefers_p1() {
    let m = facts(
        r#"{"panes": [
            {"pane_id": "wJ:p2", "workspace_id": "wJ"},
            {"pane_id": "wJ:p1", "workspace_id": "wJ"}]}"#,
    );
    assert_eq!(pick_first_pane(&m, "wJ"), Some("wJ:p1".to_string()));
}

#[test]
fn test_pick_first_pane_ignores_other_ws() {
    let m = facts(
        r#"{"panes": [
            {"pane_id": "w8:p1", "workspace_id": "w8"},
            {"pane_id": "wJ:p3", "workspace_id": "wJ"}]}"#,
    );
    assert_eq!(pick_first_pane(&m, "wJ"), Some("wJ:p3".to_string()));
}

#[test]
fn test_pick_first_pane_none_when_empty() {
    let m = facts(r#"{"panes": [{"pane_id": "w8:p1", "workspace_id": "w8"}]}"#);
    assert_eq!(pick_first_pane(&m, "wJ"), None);
}

#[test]
fn test_pick_first_pane_rejects_empty_ws() {
    // Rows with a missing workspace default to "": "" must never
    // match, or an empty id could attach cross-workspace.
    let m = facts(r#"{"panes": [{"pane_id": "w8:p1"}]}"#);
    assert_eq!(pick_first_pane(&m, ""), None);
}

#[test]
fn test_pick_first_pane_none_when_unparseable() {
    let m = facts(r#"{"panes": [{"pane_id": "bogus", "workspace_id": "wJ"}]}"#);
    assert_eq!(pick_first_pane(&m, "wJ"), None);
}

#[test]
fn test_pane_num_orders_numerically() {
    assert!(pane_num("wJ:p2") < pane_num("wJ:p12"));
    assert_eq!(pane_num("bogus"), u64::MAX);
    assert_eq!(pane_num("wJ:p"), u64::MAX);
}

#[test]
fn test_parse_layout_reads_rects() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"layout": {"panes": [
            {"pane_id": "w8:p1", "rect": {"x": 0, "y": 0, "width": 211, "height": 61}},
            {"pane_id": "w8:p2", "rect": {"x": 0, "y": 0, "width": 40, "height": 60}},
            {"pane_id": "w8:p3"}]}}"#,
    )
    .unwrap();
    let m = parse_layout(&v);
    assert_eq!(m.get("w8:p1"), Some(&(211, 61)));
    assert_eq!(m.get("w8:p2"), Some(&(40, 60)));
    assert_eq!(m.get("w8:p3"), None);
}

#[test]
fn test_best_split_direction_longer_axis() {
    // Wide → right, tall → down, ties keep the historic default.
    assert_eq!(best_split_direction(211, 61), "right");
    assert_eq!(best_split_direction(40, 60), "down");
    assert_eq!(best_split_direction(80, 80), "right");
}
