//! Tests for [`super::messages`] (split: 300-line file limit).
use super::*;

#[test]
fn test_build_edit_msg_params_none_drops_keyboard() {
    // Regression: `reply_markup` omitted ⇒ Telegram REMOVES the inline
    // keyboard. Keep-intent failures must re-attach `Some(kb)` or
    // notice beside the card (`send_msg`) — never edit with `None`.
    let bare = build_edit_msg_params(1, 2, "hi", None);
    assert!(bare.get("reply_markup").is_none(), "None must omit markup");
    let kb = json!([[{"text": "ok", "callback_data": "x"}]]);
    let kept = build_edit_msg_params(1, 2, "hi", Some(&kb));
    assert_eq!(kept["reply_markup"]["inline_keyboard"], kb);
    let strip = json!([]);
    let cleared = build_edit_msg_params(1, 2, "hi", Some(&strip));
    assert_eq!(cleared["reply_markup"]["inline_keyboard"], json!([]));
}

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
    }
    // Token death needs no gone-card (handled at the session layer);
    // rights loss without a gone thread keeps the slot for retry.
    for fatal in [
        "Unauthorized",
        "Forbidden: CHAT_ADMIN_REQUIRED",
        "Forbidden: not enough rights to send text messages",
    ] {
        assert!(
            !super::edit_gone(fatal),
            "fatal needs no gone-card: {fatal}"
        );
    }
    // Blocked-bot means the card is definitely uneditable (any case —
    // errors.rs parity); rights loss without a gone thread still keeps
    // the slot for retry.
    assert!(
        super::edit_gone("Forbidden: Bot was blocked by the user"),
        "blocked must retire in any case"
    );
    // Kicked / chat-gone means the card is definitely uneditable:
    // callers must post fresh (then prune) instead of retrying a corpse
    // edit every tick forever.
    for gone in [
        "Forbidden: bot was kicked from the group chat",
        "Forbidden: bot is not a member of the chat",
        "Bad Request: chat not found",
        "Bad Request: CHAT_NOT_FOUND",
    ] {
        assert!(super::edit_gone(gone), "gone must retire: {gone}");
    }
    // Transients still retry through the gate.
    for t in ["Internal Server Error", "Bad Gateway", "timed out"] {
        assert!(TelegramClient::is_transient_msg(t), "must retry: {t}");
    }
    // Edit-gone matches every case + the constant form (a missed
    // capital retried a corpse edit every tick instead of posting fresh).
    for gone in [
        "Bad Request: message to edit not found",
        "Bad Request: Message to edit not found",
        "Bad Request: MESSAGE_TO_EDIT_NOT_FOUND",
        "Bad Request: message can't be edited",
        "Bad Request: Message can't be edited",
    ] {
        assert!(super::edit_gone(gone), "gone must retire: {gone}");
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
