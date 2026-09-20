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

#[test]
fn test_edit_fatal_errors_fail_fast_not_retried() {
    // The `try_edit_msg` gate retries only transients (send-path
    // parity): fatals must never be transient, or every card edit burns
    // 3 calls + 2s per tick fleet-wide. Covered fatals (edit_gone /
    // not-modified) return before the gate; these must fail fast in it.
    for fatal in [
        "Unauthorized",
        "Forbidden: bot was kicked from the group chat",
        "Bad Request: chat not found",
        "Forbidden: CHAT_ADMIN_REQUIRED",
    ] {
        assert!(
            !TelegramClient::is_transient_msg(fatal),
            "fatal must fail fast: {fatal}"
        );
        assert!(!super::edit_gone(fatal), "fatal needs no gone-card: {fatal}");
    }
    // Transients still retry through the gate.
    for t in ["Internal Server Error", "Bad Gateway", "timed out"] {
        assert!(TelegramClient::is_transient_msg(t), "must retry: {t}");
    }
}

#[test]
fn test_is_effect_rejection_only_effect_shaped() {
    // Effect rejections strip and retry once.
    for m in [
        "Bad Request: EFFECT_INVALID",
        "Bad Request: message effect not allowed in this chat",
        "Bad Request: Effect_invalid",
    ] {
        assert!(super::is_effect_rejection(m), "must strip: {m}");
    }
    // Bare "not allowed" without effect context is an unrelated fatal —
    // stripping would burn a second send that fails the same way.
    for m in [
        "Forbidden: not enough rights to send text messages",
        "Forbidden: bot was kicked from the group chat",
        "Bad Request: chat not found",
    ] {
        assert!(!super::is_effect_rejection(m), "must fail fast: {m}");
    }
}
