use crate::{
    handlers::{handle_callback, handle_dm_message, handle_forum_message},
    state::AppState,
    types::{NAGGED_SECS, STALE_SECS},
};
use serde_json::Value;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Stale-notice burst guard: a boot burst queues N stale messages and
/// each must not send its own "please resend" (serial and slow — fresh
/// updates stall behind the spam). One notice per (chat, thread) per
/// STALE_SECS; a later genuine stall still notifies. Pure for tests.
fn stale_notice_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|t| now.duration_since(t).as_secs() >= STALE_SECS)
        .unwrap_or(true)
}

/// Setup-note guard: same shape, daily window — an unconfigured group
/// reminds once a day, never spams, never mutes forever. Pure for tests.
fn setup_note_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|t| now.duration_since(t).as_secs() >= NAGGED_SECS)
        .unwrap_or(true)
}

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
    // Edited messages (typo-fixes) are intentionally ignored: routing
    // the edited body like a new message double-executes prompts and
    // re-mints/re-kills on command edits, and staleness would use the
    // original date (always stale). Fail-closed — resend as a new message.
    if msg.is_null() {
        // Distinguish edits (known, dropped) from truly unknown kinds.
        if !u["edited_message"].is_null() {
            println!("[tg] ignoring edited_message (resend as new)");
        } else {
            println!("[tg] ignoring unknown update kind: {kind}");
        }
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
    // Same forum gate as renames below: thread ids are small ints that
    // collide across chats — an icon edit elsewhere must never poison a
    // mapped pane (customs stick, so the glyph would wedge).
    if let Some((thread, icon)) = crate::handlers::topic_edit::parse_topic_icon_edit(msg)
        && (chat_type == "supergroup" || chat_type == "group")
        && s.cfg.forum == Some(chat_id)
        && let Some(pane) = s.topics.pane_of_thread(thread)
    {
        println!("[topics] user customized icon for {pane}: {icon}");
        s.topics.note_user_icon(&pane, &icon);
    }
    // User cleared the custom icon: drop the stored id so the watchdog
    // heals the live kind glyph (a stale custom wedges kind flips).
    if let Some(thread) = crate::handlers::topic_edit::parse_topic_icon_cleared(msg)
        && (chat_type == "supergroup" || chat_type == "group")
        && s.cfg.forum == Some(chat_id)
        && let Some(pane) = s.topics.pane_of_thread(thread)
    {
        println!("[topics] user cleared icon for {pane}");
        s.topics.clear_user_icon(&pane);
    }

    // Native forum-topic rename (service message, no text): sync the new
    // name back to the herdr pane label. Must run BEFORE the empty-text
    // return below. No stale gate: replays are idempotent via the
    // stored-title compare, and a redelivered rename heals the down-window.
    if let Some((thread, name)) = crate::handlers::topic_edit::parse_topic_edit(msg) {
        if (chat_type == "supergroup" || chat_type == "group") && s.cfg.forum == Some(chat_id) {
            crate::handlers::titles_adopt::adopt_topic_title(s, chat_id, Some(thread), &name).await;
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
        // Burst-deduped (see stale_notice_due): short lock, no await
        // inside — the map prune rides along, never a second pass.
        let at = Instant::now();
        let due = {
            let mut nagged = s.stale_nagged.lock().await;
            nagged.retain(|_, t| at.duration_since(*t).as_secs() < STALE_SECS);
            let due = stale_notice_due(nagged.get(&(chat_id, th)).copied(), at);
            if due {
                nagged.insert((chat_id, th), at);
            }
            due
        };
        if due {
            // Spawned, never awaited: the loud send sleeps up to 3×60s
            // on flood — awaiting it here would stall the sequential
            // update pump (main loop handles updates one by one) and age
            // the whole batch past STALE_SECS into a drop cascade.
            let tg = s.tg.clone();
            tokio::spawn(async move {
                tg.send_msg(
                    chat_id,
                    th,
                    "⌛️ that message arrived too late — please resend",
                    None,
                )
                .await;
            });
        }
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
            // Daily-bounded (see setup_note_due): short lock, prune
            // rides along, send outside it.
            let due = {
                let at = Instant::now();
                let mut nagged = s.nagged.lock().await;
                nagged.retain(|_, t| at.duration_since(*t).as_secs() < NAGGED_SECS);
                let due = setup_note_due(nagged.get(&chat_id).copied(), at);
                if due {
                    nagged.insert(chat_id, at);
                }
                due
            };
            if !due {
                return;
            }
            let th = msg["message_thread_id"].as_i64();
            let note = format!(
                "🤖 Connected to Herdr!\n\nTo enable per-agent forum topics, add this group to `.env`:\n`TELEGRAM_FORUM_CHAT_ID={chat_id}`"
            );
            // Spawned, never awaited (stale-notice parity): a flood-wait
            // sleep must not stall the sequential update pump.
            let tg = s.tg.clone();
            tokio::spawn(async move {
                tg.send_msg(chat_id, th, &note, None).await;
            });
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

    #[test]
    fn test_stale_notice_due_first_then_quiet_then_due() {
        let now = Instant::now();
        assert!(stale_notice_due(None, now));
        assert!(!stale_notice_due(Some(now), now));
        let recent = now - std::time::Duration::from_secs(10);
        assert!(!stale_notice_due(Some(recent), now));
        let old = now - std::time::Duration::from_secs(STALE_SECS + 1);
        assert!(stale_notice_due(Some(old), now));
    }

    #[test]
    fn test_setup_note_due_daily_window() {
        let now = Instant::now();
        assert!(setup_note_due(None, now));
        assert!(!setup_note_due(Some(now), now));
        let hour = now - std::time::Duration::from_secs(3600);
        assert!(!setup_note_due(Some(hour), now));
        let old = now - std::time::Duration::from_secs(NAGGED_SECS + 1);
        assert!(setup_note_due(Some(old), now));
    }
}
