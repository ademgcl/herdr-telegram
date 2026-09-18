use crate::{
    handlers::{handle_callback, handle_dm_message, handle_forum_message},
    state::AppState,
    types::STALE_SECS,
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Strip newlines from user-controlled titles before logging: logged
/// titles must never forge log lines.
fn log_safe(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

pub async fn handle_update(s: AppState, u: &Value) {
    let kind = if u.get("callback_query").is_some() {
        "callback"
    } else if u.get("my_chat_member").is_some() {
        "my_chat_member"
    } else if u.get("message").is_some() {
        "message"
    } else if u.get("edited_message").is_some() {
        "edited_message"
    } else {
        "unknown"
    };
    println!("[tg] update {kind}");
    if u.get("callback_query").is_some() {
        handle_callback(s, &u["callback_query"]).await;
        return;
    }

    if let Some(member) = u.get("my_chat_member") {
        let from = member["from"]["id"].as_i64().unwrap_or(0);
        let chat = &member["chat"];
        let title = chat["title"].as_str().unwrap_or("");
        // Read the actual membership change: a kick/leave must not log as
        // an add (the removal case is when this log matters most).
        let status = member["new_chat_member"]["status"].as_str().unwrap_or("?");
        if s.cfg.owners.contains(&from) {
            // Forensics need the group name, not numeric IDs: titles
            // suffice, IDs stay out of the log.
            println!(
                "[telegram] bot membership {status} in group '{}'",
                log_safe(title)
            );
        }
        return;
    }

    let msg = &u["message"];
    if msg.is_null() {
        println!("[tg] ignoring unknown update kind: {kind}");
        return;
    }

    let from = msg["from"]["id"].as_i64();
    let chat_id = msg["chat"]["id"].as_i64();
    let chat_type = msg["chat"]["type"].as_str().unwrap_or("");
    let text = msg["text"]
        .as_str()
        .or_else(|| msg["caption"].as_str())
        .unwrap_or("");

    let (Some(from), Some(chat_id)) = (from, chat_id) else {
        return;
    };

    if !s.cfg.owners.contains(&from) {
        return; // Silently ignore non-owners
    }

    // User customized topic icon in Telegram: persist so bot never overwrites it.
    if let Some((thread, icon)) = crate::handlers::topic_edit::parse_topic_icon_edit(msg)
        && let Some(pane) = s.topics.pane_of_thread(thread)
    {
        println!("[topics] user customized icon for {pane}: {icon}");
        s.topics.note_user_icon(&pane, &icon);
    }

    // Native forum-topic rename (service message, no text): sync the new
    // name back to the herdr pane label. Must run BEFORE the empty-text
    // return below. No stale gate: replays are idempotent via the
    // stored-title compare, and a redelivered rename heals the down-window.
    if let Some((thread, name)) = crate::handlers::topic_edit::parse_topic_edit(msg) {
        if (chat_type == "supergroup" || chat_type == "group") && s.cfg.forum == Some(chat_id) {
            crate::handlers::titles::adopt_topic_title(s, chat_id, Some(thread), &name).await;
        }
        return;
    }

    if msg.get("forum_topic_edited").is_some() {
        return;
    }

    if text.is_empty() {
        return;
    }

    let date = msg["date"].as_u64().unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(date) > STALE_SECS {
        // Visible, not silent: a pump stall (long shell/model/quit run)
        // or bot downtime can age queued prompts past the gate — the
        // sender must know to resend instead of assuming delivery.
        println!("[tg] dropping stale update");
        let th = msg["message_thread_id"].as_i64();
        s.tg.send_msg(chat_id, th, "⌛️ that message arrived too late — please resend", None)
            .await;
        return;
    }

    if chat_type == "private" && chat_id == from {
        handle_dm_message(s, chat_id, msg).await;
    } else if chat_type == "supergroup" || chat_type == "group" {
        if let Some(forum_id) = s.cfg.forum {
            if chat_id == forum_id {
                handle_forum_message(s, chat_id, msg).await;
            }
        } else {
            println!(
                "[telegram] message in group '{}' without TELEGRAM_FORUM_CHAT_ID (see the setup note posted there)",
                log_safe(msg["chat"]["title"].as_str().unwrap_or(""))
            );
            if !s.nagged.lock().await.insert(chat_id) {
                return;
            }
            let th = msg["message_thread_id"].as_i64();
            let note = format!(
                "🤖 Connected to Herdr!\n\nTo enable per-agent forum topics, add this group to `.env`:\n`TELEGRAM_FORUM_CHAT_ID={chat_id}`"
            );
            s.tg.send_msg(chat_id, th, &note, None).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_safe_strips_newlines() {
        assert_eq!(log_safe("plain"), "plain");
        assert_eq!(log_safe("a\nb\rc"), "a b c");
        assert_eq!(log_safe("[x]\nFAKE LOG"), "[x] FAKE LOG");
    }
}
