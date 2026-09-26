use crate::state::AppState;
use serde_json::Value;

/// Strip a `@BotName` mention suffix from a command (`/model@MyBot` → `/model`).
/// Group clients append it when only one bot is present; plain commands pass through.
pub(crate) fn bare_cmd(cmd: &str) -> &str {
    cmd.split('@').next().unwrap_or(cmd)
}

/// Orphan-thread verdict (pure, tested): `true` when a threaded
/// message maps to nothing — a reset/remint orphan or a user-made
/// topic, never General traffic (General carries no thread id; thread
/// 1 is General when Telegram includes it). Single source for the
/// refuse below so the shape can never drift.
pub(crate) fn is_orphan_thread(thread_id: Option<i64>, pane_found: bool) -> bool {
    match thread_id {
        // Thread 1 is the General topic — never an orphan.
        Some(1) | None => false,
        Some(_) => !pane_found,
    }
}

/// Waiter key (pure, tested): General arrives as `None` on user
/// messages but `Some(1)` on callback message objects — both are the
/// same conversation, so arms keyed `Some(1)` could never meet consumes
/// keyed `None` (answer fell through, /cancel reported cancel while the
/// waiter survived). Single source for every `(chat, thread)` waiter
/// key so the shape can never drift again.
pub(crate) fn waiter_key(chat: i64, thread: Option<i64>) -> (i64, Option<i64>) {
    (chat, thread.filter(|t| *t != 1))
}

pub async fn handle_forum_message(s: AppState, chat: i64, msg: &Value) {
    let from = msg["from"]["id"].as_i64().unwrap_or(0);
    if !s.cfg.owners.contains(&from) {
        return;
    }

    let thread_id = msg["message_thread_id"].as_i64();
    let text = msg["text"]
        .as_str()
        .or_else(|| msg["caption"].as_str())
        .unwrap_or("")
        .trim();
    // Photos route with the image attached (empty caption still
    // prompts — the marker alone describes what arrived). Only
    // truly content-free updates drop here.
    let photo_id = crate::telegram::photo::largest_file_id(&msg["photo"]);
    if text.is_empty() && photo_id.is_none() {
        return;
    }

    // `/space [name]` works everywhere (General + any agent/shell topic):
    // new space + shell topic to keep chatting in. Blank auto-labels.
    // Waiter precedence: an armed typed/keyed/run answer for THIS thread
    // owns the next message (DM parity) — a literal "/space …" answer
    // must answer, never mint a billable workspace mid-dialog. Armed
    // threads (mapped or General) skip the mint and fall through to the
    // topic/General dispatch below, which consumes waiters first. (Keys
    // go through `waiter_key`: General None and Some(1) meet.)
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    if bare_cmd(raw_cmd) == "/space" {
        // Fail-closed: check the normalized waiter key unconditionally —
        // a thread-less General update can still own a waiter armed from
        // a callback whose message carried thread 1 (panel taps carry no
        // thread id), and minting is billable.
        let k = waiter_key(chat, thread_id);
        let armed_here = s.typewait.lock().await.contains_key(&k)
            || s.keywait.lock().await.contains_key(&k)
            || s.runwait.lock().await.contains_key(&k);
        if !armed_here {
            let label = if super::space::check_label(arg) {
                arg.to_string()
            } else {
                // Auto-label needs a readable list: an outage refuses
                // before any billable mint, never a colliding `space-1`.
                let Some(label) = super::space::next_label(&s).await else {
                    s.tg.send_msg(chat, thread_id, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                };
                label
            };
            super::space::open_space(&s, chat, thread_id, &label).await;
            return;
        }
    }

    // If inside an agent topic thread:
    if let Some(th) = thread_id
        && let Some(pane) = s.topics.pane_of_thread(th)
    {
        if let Some(mid) = msg["message_id"].as_i64() {
            s.topics.record_msg(&pane, mid);
        }
        // Bodies stay out of the log (prompts can carry pasted
        // secrets); pane + length suffice for traffic forensics.
        println!(
            "[forum] topic msg for {pane} ({} chars)",
            text.chars().count()
        );
        super::forum_topic::handle_topic_agent_message(
            s,
            chat,
            th,
            &pane,
            text,
            photo_id.as_deref(),
        )
        .await;
        return;
    }

    // Inside General topic or non-agent thread:
    // Orphan refuse (status/model parity): a threaded message whose
    // thread maps to nothing is a reset/remint orphan or a user-made
    // topic — never General traffic (no thread id, or thread 1).
    // General control (`/reset`, `/spawn`, …) must never fire from a
    // corpse replay, and bare prompts must never route with the wrong
    // id for follow-ups. Posted in-thread: dead threads fail the send
    // silently (same as a drop), live ones get guidance (no silent
    // loss). Never falls through to General routing.
    // Mapped threads returned early above: feed the live lookup into
    // the verdict (single source) instead of a hardcoded flag, so the
    // two can never drift apart.
    let pane_found = thread_id.is_some_and(|th| s.topics.pane_of_thread(th).is_some());
    if is_orphan_thread(thread_id, pane_found) {
        s.tg.send_msg(chat, thread_id, crate::ui::UNKNOWN_TOPIC, None)
            .await;
        return;
    }
    // General is control-plane only: photos are not commands — refuse
    // visibly instead of dropping silently.
    if photo_id.is_some() {
        s.tg.send_msg(chat, thread_id, crate::ui::PHOTO_HINT_GENERAL, None)
            .await;
        return;
    }
    super::general::handle_general_forum_message(s, chat, thread_id, text).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_cmd_strips_mention() {
        assert_eq!(bare_cmd("/model@HerdrBot"), "/model");
        assert_eq!(bare_cmd("/model"), "/model");
        assert_eq!(bare_cmd("hello"), "hello");
    }

    #[test]
    fn test_waiter_key_general_parity() {
        // General arrives as None on messages but Some(1) on callbacks —
        // one key so arms meet consumes (answer + /cancel + typewait).
        assert_eq!(waiter_key(7, None), waiter_key(7, Some(1)));
        assert_eq!(waiter_key(7, None), (7, None));
        // Real topic threads keep their identity.
        assert_eq!(waiter_key(7, Some(9)), (7, Some(9)));
    }

    #[test]
    fn test_is_orphan_thread_live_only() {
        // Threaded + unmapped (remint orphan / user-made): refuse.
        assert!(is_orphan_thread(Some(7), false));
        // General (no thread, or thread 1) never refuses, even unmapped.
        assert!(!is_orphan_thread(None, false));
        assert!(!is_orphan_thread(Some(1), false));
        // Mapped agent/shell threads route to their topic, never refuse.
        assert!(!is_orphan_thread(Some(7), true));
        assert!(!is_orphan_thread(None, true));
    }
}
