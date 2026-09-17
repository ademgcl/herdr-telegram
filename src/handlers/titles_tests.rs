//! Tests for [`super::parse_topic_edit`] + [`super::parse_topic_icon_edit`]
//! (split: 300-line file limit).
use super::*;
use serde_json::json;

fn edit_msg(thread: Option<i64>, name: &str) -> Value {
    let mut m = json!({"message_thread_id": 17, "forum_topic_edited": {"name": name}});
    if let Some(th) = thread {
        m["message_thread_id"] = json!(th);
    } else {
        m.as_object_mut().unwrap().remove("message_thread_id");
    }
    m
}

#[test]
fn test_parse_topic_edit_service_msg() {
    assert_eq!(
        parse_topic_edit(&edit_msg(Some(17), "o2 · myblender")),
        Some((17, "o2 · myblender".to_string()))
    );
    // Padded names trim.
    assert_eq!(
        parse_topic_edit(&edit_msg(Some(17), "  api  ")),
        Some((17, "api".to_string()))
    );
}

#[test]
fn test_parse_topic_edit_rejects_non_edits() {
    // Plain text message: no forum_topic_edited key.
    assert_eq!(parse_topic_edit(&json!({"text": "/space x"})), None);
    // Missing thread or blank name: unroutable.
    assert_eq!(parse_topic_edit(&edit_msg(None, "api")), None);
    assert_eq!(parse_topic_edit(&edit_msg(Some(17), "   ")), None);
    assert_eq!(
        parse_topic_edit(&json!({"message_thread_id": 17, "forum_topic_edited": {}})),
        None
    );
}

#[test]
fn test_parse_topic_icon_edit() {
    let msg = json!({
        "message_thread_id": 17,
        "forum_topic_edited": {"icon_custom_emoji_id": "5350554349074391003"}
    });
    assert_eq!(
        parse_topic_icon_edit(&msg),
        Some((17, "5350554349074391003".to_string()))
    );
    assert_eq!(
        parse_topic_icon_edit(
            &json!({"message_thread_id": 17, "forum_topic_edited": {"name": "hi"}})
        ),
        None
    );
}
