//! Tests for [`super::messages`] (split: 300-line file limit).
use super::*;

#[test]
fn test_build_send_msg_params_effect() {
    let p1 = build_send_msg_params(123, Some(456), "hello", None, Some(EFFECT_FIRE), false);
    assert_eq!(p1["chat_id"], 123);
    assert_eq!(p1["message_thread_id"], 456);
    assert_eq!(p1["message_effect_id"], EFFECT_FIRE);
    assert!(p1.get("disable_notification").is_none());

    let p2 = build_send_msg_params(123, None, "hello", None, None, false);
    assert_eq!(p2["chat_id"], 123);
    assert!(p2.get("message_thread_id").is_none());
    assert!(p2.get("message_effect_id").is_none());

    let p3 = build_send_msg_params(123, None, "hello", None, None, true);
    assert_eq!(p3["disable_notification"], true);
}
