use super::client::TelegramClient;
use crate::types::Res;
use serde_json::json;
use std::time::Duration;

/// True when a create error rejects the icon color — retry without it.
/// Case-insensitive (`COLOR_INVALID`, `Icon_color_invalid`, …).
/// Color-shaped only: a bare "color" substring also matches unrelated
/// fatals and would mask them behind a doomed retry. Single source for
/// the strip gate below.
pub fn is_color_rejection(msg: &str) -> bool {
    msg.contains("COLOR_INVALID") || msg.to_lowercase().contains("icon_color")
}

pub fn build_create_forum_topic_params(
    chat_id: i64,
    name: &str,
    icon_color: Option<i64>,
) -> serde_json::Value {
    let mut body = json!({"chat_id": chat_id, "name": name});
    if let Some(color) = icon_color {
        body["icon_color"] = json!(color);
    }
    body
}

impl TelegramClient {
    /// F5: Create a forum topic with optional icon_color (one of Telegram's 6 RGB values).
    pub async fn create_forum_topic(
        &self,
        chat_id: i64,
        name: &str,
        icon_color: Option<i64>,
    ) -> Res<i64> {
        let mut body = build_create_forum_topic_params(chat_id, name, icon_color);
        let res = match self
            .call_retrying("createForumTopic", body.clone(), Duration::from_secs(15))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                // Fail-closed shape: body is locally built as an object —
                // a non-object here returns the error instead of panicking.
                if body.get("icon_color").is_some() && is_color_rejection(&msg) {
                    let Some(obj) = body.as_object_mut() else {
                        return Err(e);
                    };
                    obj.remove("icon_color");
                    self.call_retrying("createForumTopic", body, Duration::from_secs(15))
                        .await?
                } else {
                    return Err(e);
                }
            }
        };
        res["message_thread_id"]
            .as_i64()
            .ok_or_else(|| "missing message_thread_id in createForumTopic response".into())
    }

    pub async fn delete_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "deleteForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                // Converged like every sibling prune site (probe,
                // reopen/close): kicked/chat-gone deletes nothing, so
                // they succeed instead of logging orphan noise.
                if super::errors::topic_gone(&msg) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    /// Close: corpse propagates (caller prunes by thread) — never
    /// swallowed here, or the next caller treats `true` as done and
    /// leaks a dead thread forever. Already-closed converges like
    /// reopen's already-open (reopen parity): without this every 60s
    /// watchdog tick errors + retries a closed topic forever.
    pub async fn close_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "closeForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                if super::errors::is_close_converged(&e.to_string()) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    pub async fn reopen_forum_topic(&self, chat_id: i64, thread_id: i64) -> Res<()> {
        match self
            .call_retrying(
                "reopenForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                // Corpse propagates so the caller prunes by thread;
                // already-open states stay Ok.
                if super::errors::topic_missing(&msg) {
                    return Err(e);
                }
                // Already-open converges (shared helper, close parity).
                if super::errors::is_close_converged(&msg) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    /// Rename a forum topic — the herdr→telegram half of 1:1 title
    /// sync. Silent (never notifies); our own edit echoes back as
    /// `forum_topic_edited`, which the stored-title compare absorbs.
    /// A no-op rename is converged, not a failure (set_topic_icon
    /// parity via the shared helper — never retry-spam the tick).
    pub async fn set_topic_title(&self, chat_id: i64, thread_id: i64, name: &str) -> Res<()> {
        match self
            .call_retrying(
                "editForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id, "name": name}),
                Duration::from_secs(15),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if super::errors::topic_not_modified(&e.to_string()) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Set a forum topic's custom-emoji icon — the silent state signal.
    /// Unlike `icon_color` (create-only, ignored on edit), this applies
    /// AND renders on edit (verified live). Never notifies.
    pub async fn set_topic_icon(&self, chat_id: i64, thread_id: i64, emoji_id: &str) -> Res<()> {
        if let Err(e) = self
            .call_retrying(
                "editForumTopic",
                json!({"chat_id": chat_id, "message_thread_id": thread_id, "icon_custom_emoji_id": emoji_id}),
                Duration::from_secs(15),
            )
            .await
        {
            let msg = e.to_string();
            // No-op edit converges (shared helper — never retry-spam).
            if super::errors::topic_not_modified(&msg) {
                return Ok(());
            }
            eprintln!("set_topic_icon #{thread_id} failed: {}", self.redact(&msg));
            return Err(msg.into());
        }
        Ok(())
    }

    /// F4: Fetch valid custom-emoji sticker IDs for forum topic icons.
    pub async fn get_forum_topic_icon_stickers(&self) -> Res<Vec<String>> {
        let res = self
            .call_retrying(
                "getForumTopicIconStickers",
                json!({}),
                Duration::from_secs(15),
            )
            .await?;
        let stickers = res
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s["custom_emoji_id"].as_str().map(|id| id.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(stickers)
    }

    /// F9: Probe bot admin rights in the forum supergroup (`can_manage_topics`).
    pub async fn check_forum_permissions(&self, chat_id: i64) -> Res<BotPermissions> {
        let me = self.get_me().await?;
        let bot_id = me["id"]
            .as_i64()
            .ok_or("missing bot id in getMe response")?;
        let member = self
            .call_retrying(
                "getChatMember",
                json!({"chat_id": chat_id, "user_id": bot_id}),
                Duration::from_secs(15),
            )
            .await?;
        let status = member["status"].as_str().unwrap_or("");
        let is_admin = status == "administrator" || status == "creator";
        // A `creator` bot omits the can_* flags (implicit full rights):
        // defaulting absent flags to false would false-WARN and wrongly
        // refuse gates keyed on them. Absent reads as admin iff admin.
        let can_manage_topics = member["can_manage_topics"].as_bool().unwrap_or(is_admin);
        let can_delete_messages = member["can_delete_messages"].as_bool().unwrap_or(is_admin);
        Ok(BotPermissions {
            is_admin,
            can_manage_topics,
            can_delete_messages,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotPermissions {
    pub is_admin: bool,
    pub can_manage_topics: bool,
    pub can_delete_messages: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_create_forum_topic_params() {
        let p1 = build_create_forum_topic_params(123, "test-topic", Some(0x6FB9F0));
        assert_eq!(p1["chat_id"], 123);
        assert_eq!(p1["name"], "test-topic");
        assert_eq!(p1["icon_color"], 0x6FB9F0);

        let p2 = build_create_forum_topic_params(123, "test-topic", None);
        assert_eq!(p2["chat_id"], 123);
        assert_eq!(p2["name"], "test-topic");
        assert!(p2.get("icon_color").is_none());
    }

    #[test]
    fn test_is_color_rejection_only_color_shaped() {
        for m in [
            "Bad Request: COLOR_INVALID",
            "Bad Request: Icon_color_invalid",
        ] {
            assert!(is_color_rejection(m), "must strip: {m}");
        }
        // "color" as a coincidental substring must not mask the real fatal.
        for m in [
            "Bad Request: TOPIC_ID_INVALID",
            "Forbidden: bot was kicked from the group chat",
        ] {
            assert!(!is_color_rejection(m), "must propagate: {m}");
        }
    }
}
