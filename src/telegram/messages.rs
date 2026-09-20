//! Telegram send-message params + silent/buzz sends.
use super::client::TelegramClient;
use crate::{
    types::{LIVE_RPC_TIMEOUT_SECS, Res},
    ui::fit_msg,
};
use serde_json::{Value, json};
use std::time::Duration;

/// Telegram animated message effect ID for fire/flame (urgent alerts: blocked, limit stall).
pub const EFFECT_FIRE: &str = "5104841245755180586";

/// True when a send error rejects the message effect (unsupported chat,
/// bad effect id) — strip `message_effect_id` and retry once.
/// Effect-shaped only: a bare "not allowed" also matches unrelated fatals
/// (rights/kicked) that must fail fast below instead of burning a second
/// send that fails the same way. Single source for the strip gate.
pub fn is_effect_rejection(msg: &str) -> bool {
    msg.to_lowercase().contains("effect")
}

/// True when an edit error means the card is definitely gone or
/// uneditable (deleted topic/thread, removed message, lost rights) —
/// callers may post a fresh card without duplicating a live one.
/// Any other error (timeout, flood-wait exhaustion, network) leaves
/// the card plausibly alive: callers must keep the slot and retry the
/// edit, never send fresh. Single source for the fatal match below.
pub fn edit_gone(msg: &str) -> bool {
    super::errors::topic_gone(msg)
        || msg.contains("message to edit not found")
        || msg.contains("Message to edit not found")
        || msg.contains("MESSAGE_TO_EDIT_NOT_FOUND")
        || msg.contains("message can't be edited")
        || msg.contains(super::errors::NO_RIGHTS)
        || msg.contains(super::errors::BOT_BLOCKED)
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

impl TelegramClient {
    pub async fn send_msg(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
        keyboard: Option<Value>,
    ) -> Option<i64> {
        self.send_msg_with_effect(chat_id, thread_id, text, keyboard, None)
            .await
    }

    /// Silent send (no buzz): progress the user watches, not hears.
    /// Single attempt with a short deadline (a missed stream send is
    /// harmless — output adopts the slot, folds retire it); None on any
    /// failure.
    /// The deadline IS the bound — callers await this directly with no
    /// outer timeout, so a card delivered before the deadline is always
    /// tracked (an outer timeout firing first would orphan an untracked
    /// live card that the next tick duplicates with a fresh send).
    /// Residual race: a send landing server-side after the deadline
    /// still reads as a miss and retries fresh (Telegram has no
    /// idempotency key) — rare against a 4s local send, and confined to
    /// sends; edits always retry in place and never duplicate.
    pub async fn send_silent(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Option<i64> {
        let params = build_send_msg_params(chat_id, thread_id, text, None, None, true);
        match self
            .call(
                "sendMessage",
                params,
                Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
            )
            .await
        {
            Ok(v) => v["message_id"].as_i64(),
            Err(e) => {
                eprintln!("sendMessage silent failed: {}", self.redact(&e.to_string()));
                None
            }
        }
    }

    /// F8: Send message with optional message effect ID (e.g. fire/flame for urgent alerts).
    /// If Telegram rejects the effect (e.g. in unsupported chats), retries without effect.
    pub async fn send_msg_with_effect(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
        keyboard: Option<Value>,
        effect_id: Option<&str>,
    ) -> Option<i64> {
        let mut params =
            build_send_msg_params(chat_id, thread_id, text, keyboard, effect_id, false);
        // Single shared budget across flood-waits, effect-strip and
        // transient retries (≤6 retries total).
        let mut used = 0;
        loop {
            match self
                .call("sendMessage", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(v) => return v["message_id"].as_i64(),
                Err(e) => {
                    let msg = e.to_string();
                    // Flood-wait always wins over the effect strip below.
                    if let Some(wait) = Self::retry_after(&msg) {
                        used += 1;
                        if used > 6 {
                            eprintln!("sendMessage failed: {}", self.redact(&msg));
                            break;
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if params.get("message_effect_id").is_some() && is_effect_rejection(&msg) {
                        let Some(obj) = params.as_object_mut() else {
                            eprintln!("sendMessage failed: {}", self.redact(&msg));
                            break;
                        };
                        obj.remove("message_effect_id");
                        used += 1;
                        if used > 6 {
                            eprintln!("sendMessage failed: {}", self.redact(&msg));
                            break;
                        }
                        continue;
                    }
                    // Telegram 5xx arrives as plain strings via `call`
                    // (never a reqwest downcast match): same transient
                    // set as `call_retrying` or buzz alerts drop silently.
                    let retryable =
                        Self::is_transport_transient(e.as_ref()) || Self::is_transient_msg(&msg);
                    used += 1;
                    if !retryable || used > 6 {
                        eprintln!("sendMessage failed: {}", self.redact(&msg));
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        None
    }

    pub async fn edit_msg(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<Value>,
    ) {
        let _ = self.try_edit_msg(chat_id, message_id, text, keyboard).await;
    }

    /// Fallible edit for live/final cards: caller falls back to a fresh
    /// send when the message is gone (deleted topic, etc.).
    pub async fn try_edit_msg(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<Value>,
    ) -> Res<()> {
        let body = fit_msg(text);
        let mut params = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": body,
        });
        if let Some(kb) = keyboard {
            params["reply_markup"] = json!({"inline_keyboard": kb});
        }
        let mut sends = 0;
        let mut waits = 0;
        loop {
            match self
                .call("editMessageText", params.clone(), Duration::from_secs(15))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if super::errors::topic_not_modified(&msg) {
                        return Ok(());
                    }
                    if edit_gone(&msg) {
                        eprintln!("editMessageText fatal: {}", self.redact(&msg));
                        return Err(self.redact(&msg).into());
                    }
                    if let Some(wait) = Self::retry_after(&msg) {
                        waits += 1;
                        if waits > 3 {
                            return Err(self.redact(&msg).into());
                        }
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    sends += 1;
                    // Send-path parity: fatals (revoked token, kicked,
                    // rights loss) fail fast — retrying them burns 3 calls
                    // + 2s per card per tick fleet-wide.
                    let retryable =
                        Self::is_transport_transient(e.as_ref()) || Self::is_transient_msg(&msg);
                    if !retryable || sends >= 3 {
                        eprintln!("editMessageText failed: {}", self.redact(&msg));
                        return Err(self.redact(&msg).into());
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// "typing…" indicator — lasts ~5s, repeat to sustain. Zero clutter.
    /// 5s timeout (not 10s): sustain loops re-fire well inside expiry,
    /// so a slow send must not stretch the period past it and flicker.
    pub async fn typing(&self, chat_id: i64, thread_id: Option<i64>) {
        let mut params = json!({"chat_id": chat_id, "action": "typing"});
        if let Some(th) = thread_id {
            params["message_thread_id"] = json!(th);
        }
        let _ = self
            .call("sendChatAction", params, Duration::from_secs(5))
            .await;
    }

    pub async fn answer_callback(&self, cbq_id: &str) {
        let _ = self
            .call(
                "answerCallbackQuery",
                json!({"callback_query_id": cbq_id}),
                Duration::from_secs(10),
            )
            .await;
    }

    /// F6: Copy a message to another topic thread without forward headers.
    pub async fn copy_msg(
        &self,
        chat_id: i64,
        from_chat_id: i64,
        message_id: i64,
        thread_id: Option<i64>,
    ) -> Option<i64> {
        let mut params = json!({
            "chat_id": chat_id,
            "from_chat_id": from_chat_id,
            "message_id": message_id,
        });
        if let Some(th) = thread_id {
            params["message_thread_id"] = json!(th);
        }
        match self
            .call_retrying("copyMessage", params, Duration::from_secs(15))
            .await
        {
            Ok(v) => v["message_id"].as_i64(),
            Err(e) => {
                eprintln!(
                    "copyMessage {message_id} failed: {}",
                    self.redact(&e.to_string())
                );
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
