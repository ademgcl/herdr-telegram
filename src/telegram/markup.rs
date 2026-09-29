//! Markup-only card edits (editMessageReplyMarkup): claim/strip buttons
//! without touching text. Split from `messages` (500-line file limit).
//!
//! Strips are single-attempt best-effort (no retry): they run inside the
//! tap's single-flight hold, so a degraded-Telegram retry storm must
//! never wedge the pane behind a doomed UX call. Convergence never
//! depends on a strip landing — every outcome arm overwrites the card
//! (or the delayed heal re-renders it).
use super::client::TelegramClient;
use crate::ui::fit_msg;
use serde_json::{Value, json};
use std::time::Duration;

pub fn build_markup_params(chat_id: i64, message_id: i64, keyboard: Option<Value>) -> Value {
    let mut params = json!({"chat_id": chat_id, "message_id": message_id});
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    params
}

pub fn build_send_msg_params(
    chat_id: i64,
    thread_id: Option<i64>,
    text: &str,
    keyboard: Option<Value>,
    effect_id: Option<&str>,
    silent: bool,
) -> Value {
    let body = fit_msg(text);
    let mut params = json!({"chat_id": chat_id, "text": body});
    if let Some(th) = thread_id {
        params["message_thread_id"] = json!(th);
    }
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    if let Some(eff) = effect_id {
        params["message_effect_id"] = json!(eff);
    }
    if silent {
        params["disable_notification"] = json!(true);
    }
    params
}

/// `editMessageText` params (pure, tested): `keyboard: None` OMITS
/// `reply_markup`, and Telegram then DROPS the inline keyboard — keep-
/// intent failures must re-attach a keyboard or notice beside the card.
pub fn build_edit_msg_params(
    chat_id: i64,
    message_id: i64,
    text: &str,
    keyboard: Option<&Value>,
) -> Value {
    let body = fit_msg(text);
    let mut params = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": body,
    });
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    params
}

/// Params for the resolve edit. Split out so the buttons-off guarantee is
/// testable without an RPC.
pub(crate) fn resolve_params(chat_id: i64, message_id: i64, text: &str) -> Value {
    let mut params = build_markup_params(chat_id, message_id, Some(Value::Array(Vec::new())));
    params["text"] = json!(fit_msg(&answered_text(text)));
    params
}

/// Rewrite a dialog card into its answered form: the "needs input"
/// header and either tap hint become a resolved marker, and the question
/// survives as a record of what was asked.
pub fn answered_text(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with("⛔ blocked")
                && !t.starts_with("Tap an answer")
                && !t.starts_with("Couldn't read the options")
        })
        .collect();
    let body = kept.join("\n");
    if body.trim().is_empty() {
        return "✅ answered".to_string();
    }
    format!("✅ answered\n\n{body}")
}

impl TelegramClient {
    /// Mark a dialog card ANSWERED: rewrite its text and drop its
    /// buttons in one `editMessageText`. An empty `inline_keyboard` is
    /// exactly what `strip_buttons` sends and Telegram removes the
    /// keyboard on it, so one call does both; on failure fall back to a
    /// markup-only strip so live buttons never outlive their dialog.
    pub async fn resolve_card(&self, chat_id: i64, message_id: i64, text: &str) {
        let params = resolve_params(chat_id, message_id, text);
        if self
            .call("editMessageText", params, Duration::from_secs(10))
            .await
            .is_err()
        {
            self.strip_buttons(chat_id, message_id).await;
        }
    }

    /// Best-effort button strip: one attempt, never fails the caller.
    /// Pending claims and fail-closed converges share it.
    pub async fn strip_buttons(&self, chat_id: i64, message_id: i64) {
        let params = build_markup_params(chat_id, message_id, Some(Value::Array(Vec::new())));
        let _ = self
            .call("editMessageReplyMarkup", params, Duration::from_secs(10))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_markup_params_strips_with_empty_kb() {
        let p = build_markup_params(1, 2, Some(Value::Array(Vec::new())));
        assert_eq!(p["chat_id"], 1);
        assert_eq!(p["message_id"], 2);
        assert_eq!(
            p["reply_markup"]["inline_keyboard"],
            Value::Array(Vec::new())
        );
        let q = build_markup_params(1, 2, None);
        assert!(q.get("reply_markup").is_none());
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::{answered_text, resolve_params};
    use serde_json::json;

    const CARD: &str = "⛔ blocked — needs input\n\nRecommended models\nClaude Sonnet 5 (default)\n\nTap an answer.";

    #[test]
    fn test_answered_card_stops_claiming_it_needs_input() {
        let out = answered_text(CARD);
        for stale in ["⛔ blocked", "needs input", "Tap an answer"] {
            assert!(!out.contains(stale), "still says {stale:?}: {out:?}");
        }
        assert!(
            out.contains("Claude Sonnet 5"),
            "lost the question: {out:?}"
        );
        assert!(out.starts_with("✅ answered"), "{out:?}");
    }

    #[test]
    fn test_unreadable_options_hint_is_also_dropped() {
        let c = "⛔ blocked — needs input\n\nPick one\n\nCouldn't read the options off the pane — type your answer, or `/read` to see it raw.";
        let out = answered_text(c);
        assert!(!out.contains("Couldn't read"), "{out:?}");
        assert!(out.contains("Pick one"), "{out:?}");
    }

    /// The buttons-off guarantee rides on the SAME edit: an empty
    /// `inline_keyboard` is what removes them, and omitting `reply_markup`
    /// entirely would leave dead ⛔ buttons on a resolved card. Pinned by
    /// inspecting the params the edit is built from.
    #[test]
    fn test_resolve_edit_carries_an_empty_keyboard_and_the_new_text() {
        let p = resolve_params(4242, 77, "⛔ blocked — needs input\n\nPick one");
        assert_eq!(p["chat_id"], json!(4242));
        assert_eq!(p["message_id"], json!(77));
        assert_eq!(
            p["reply_markup"]["inline_keyboard"],
            json!([]),
            "an empty keyboard is what removes the buttons"
        );
        let text = p["text"].as_str().unwrap();
        assert!(!text.contains("⛔ blocked"), "{text}");
        assert!(text.contains("Pick one"), "question lost: {text}");
    }

    #[test]
    fn test_empty_or_headerless_still_resolves_cleanly() {
        assert_eq!(answered_text(""), "✅ answered");
        assert_eq!(
            answered_text("⛔ blocked — needs input\n\nTap an answer."),
            "✅ answered"
        );
    }
}
