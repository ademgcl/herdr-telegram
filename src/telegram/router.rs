use super::router_guards::{log_safe, setup_note_due, stale_notice_due};
use crate::{
    handlers::{handle_callback, handle_dm_message, handle_forum_message},
    state::AppState,
    types::{NAGGED_SECS, STALE_SECS},
};
use serde_json::Value;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

    // Service-edit freshness (icon arms below persist customs — a write):
    // a redelivered `forum_topic_edited` would re-stamp an old custom
    // (wedging kind-heal until the next manual clear). Stale or dateless
    // icon edits drop — fail-closed, no write on ambiguous (renames stay
    // fresh-agnostic: see below. Text re-checks with its loud notice).
    let msg_fresh = msg["date"].as_u64().map(|d| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(d)
            <= STALE_SECS
    }).unwrap_or(false);

    // User customized topic icon in Telegram: persist so bot never overwrites it.
    // Same forum gate as renames below: thread ids are small ints that
    // collide across chats — an icon edit elsewhere must never poison a
    // mapped pane (customs stick, so the glyph would wedge). Reset-gated
    // like renames (adopt_topic_title): reset_topic preserves customs via
    // icon_needs_update, so a mid-reset custom wedges the mint on the
    // wrong glyph / blocks kind-heal.
    if msg_fresh
        && !crate::handlers::reset::is_resetting()
        && let Some((thread, icon)) = crate::handlers::topic_edit::parse_topic_icon_edit(msg)
        && (chat_type == "supergroup" || chat_type == "group")
        && s.cfg.forum == Some(chat_id)
        && let Some(pane) = s.topics.pane_of_thread(thread)
    {
        println!("[topics] user customized icon for {pane}: {icon}");
        s.topics.note_user_icon(&pane, &icon);
    }
    // User cleared the custom icon: drop the stored id so the watchdog
    // heals the live kind glyph (a stale custom wedges kind flips).
    // Reset-gated like the custom arm above.
    if msg_fresh
        && !crate::handlers::reset::is_resetting()
        && let Some(thread) = crate::handlers::topic_edit::parse_topic_icon_cleared(msg)
        && (chat_type == "supergroup" || chat_type == "group")
        && s.cfg.forum == Some(chat_id)
        && let Some(pane) = s.topics.pane_of_thread(thread)
    {
        println!("[topics] user cleared icon for {pane}");
        s.topics.clear_user_icon(&pane);
    }

    // Native forum-topic rename (service message, no text): sync the new
    // name back to the herdr pane label. Must run BEFORE the empty-text
    // return below. Deliberately fresh-agnostic (unlike the icon arms):
    // Telegram replays unacked updates in offset order (bot-down renames
    // arrive stale and must still heal), and adopt's stored-compare +
    // herdr_moved_on already drop dupes and herdr-side winners — a date
    // gate here would only destroy legit bot-down renames for the
    // watchdog to revert.
    if let Some((thread, name)) = crate::handlers::topic_edit::parse_topic_edit(msg)
        && (chat_type == "supergroup" || chat_type == "group")
        && s.cfg.forum == Some(chat_id)
    {
        crate::handlers::titles_adopt::adopt_topic_title(s, chat_id, Some(thread), &name).await;
        return;
    }

    if msg.get("forum_topic_edited").is_some() {
        return;
    }

    if text.is_empty() {
        return;
    }

    // Fail-closed: a message with no date is ambiguous — drop silently
    // instead of treating it as always-stale (loud) or always-fresh.
    let Some(date) = msg["date"].as_u64() else {
        return;
    };
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
        // General arrives as None on messages but Some(1) on callbacks:
        // one waiter_key so both are the same conversation (single
        // source with forum.rs — never two notices for General).
        let at = Instant::now();
        let due = {
            let key = crate::handlers::forum::waiter_key(chat_id, th);
            let mut nagged = s.stale_nagged.lock().await;
            nagged.retain(|_, t| at.duration_since(*t).as_secs() < STALE_SECS);
            let due = stale_notice_due(nagged.get(&key).copied(), at);
            if due {
                nagged.insert(key, at);
            }
            due
        };
        if due {
            // Spawned, never awaited: the loud send sleeps up to 3×60s
            // on flood — awaiting it here would stall the sequential
            // update pump (main loop handles updates one by one) and age
            // the whole batch past STALE_SECS into a drop cascade.
            // Normalized send (waiter_key parity): General arrives as
            // None on messages but Some(1) on callbacks — both are the
            // same conversation, so the notice lands the same way.
            let th_send = th.filter(|t| *t != 1);
            let tg = s.tg.clone();
            tokio::spawn(async move {
                tg.send_msg(
                    chat_id,
                    th_send,
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
            } else {
                // Owner typing in a non-forum group: same daily-bounded
                // guidance as the unconfigured arm below, never a dead
                // silent drop (a mistyped/moved group looks like a dead
                // bot otherwise).
                println!(
                    "[telegram] message in non-forum group '{}' (see guidance)",
                    log_safe(msg["chat"]["title"].as_str().unwrap_or(""))
                );
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
                let th_send = msg["message_thread_id"].as_i64().filter(|t| *t != 1);
                let note = "🤖 I only serve the configured forum group from here — please use the forum topics or DM me.";
                let tg = s.tg.clone();
                tokio::spawn(async move {
                    tg.send_msg(chat_id, th_send, note, None).await;
                });
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
            // Normalized send (waiter_key parity with the stale notice):
            // General arrives as None on messages but Some(1) on
            // callbacks — both are the same conversation.
            let th = msg["message_thread_id"].as_i64().filter(|t| *t != 1);
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
