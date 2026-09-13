/// One pinned live-status message per agent topic, edited in place.
/// Silent (no notification buzz, no unread bump, no topic-list flicker) —
/// this is where state lives now, not in topic names or alert spam.

use std::time::Duration;
use serde_json::json;
use crate::{state::AppState, ui::emoji, ui::fit_msg};

/// Pin body: status line plus the latest answer excerpt (if any).
/// Pure — tested below.
pub fn pin_text(status: &str, excerpt: &str) -> String {
    let body: String = excerpt.chars().take(800).collect();
    if body.trim().is_empty() {
        format!("{} {}", emoji(status), status)
    } else {
        format!("{} {}\n\n{}", emoji(status), status, body.trim())
    }
}

/// Refresh the pane's pinned status card: create (+silently pin) on first
/// sight, otherwise edit in place. Identical refreshes skip the edit; a
/// deleted pin is forgotten so the next refresh recreates it.
pub async fn refresh_pin(s: &AppState, pane: &str, status: &str) {
    let forum = match s.cfg.forum {
        Some(f) => f,
        None => return,
    };
    let thread = match s.topics.get_thread(pane) {
        Some(t) => t,
        None => return,
    };
    let excerpt = s.last_reply.lock().await.get(pane).cloned().unwrap_or_default();
    let text = pin_text(status, &excerpt);
    if s.pin_text.lock().await.get(pane) == Some(&text) {
        return;
    }
    match s.topics.get_pin(pane) {
        Some(mid) => {
            let res = s
                .tg
                .call(
                    "editMessageText",
                    json!({"chat_id": forum, "message_id": mid, "text": fit_msg(&text)}),
                    Duration::from_secs(15),
                )
                .await;
            match res {
                Ok(_) => {
                    s.pin_text.lock().await.insert(pane.to_string(), text);
                }
                Err(e) => {
                    let msg = e.to_string().to_lowercase();
                    if msg.contains("not modified") {
                        s.pin_text.lock().await.insert(pane.to_string(), text);
                    } else if msg.contains("not found") {
                        // Pin deleted out of band — forget it, recreate next time.
                        s.topics.clear_pin(pane);
                        s.pin_text.lock().await.remove(pane);
                    } else {
                        eprintln!("[pin] edit #{mid} ({pane}) failed: {e}");
                    }
                }
            }
        }
        None => {
            if let Some(mid) = s.tg.send_quiet(forum, Some(thread), &text).await {
                s.tg.pin_msg(forum, mid).await;
                s.topics.set_pin(pane, mid);
                s.pin_text.lock().await.insert(pane.to_string(), text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_text_with_and_without_excerpt() {
        assert_eq!(pin_text("idle", ""), "🟢 idle");
        assert_eq!(pin_text("working", "  "), "🔄 working");
        assert_eq!(
            pin_text("done", "  -2 (or the range)  "),
            "✅ done\n\n-2 (or the range)"
        );
    }

    #[test]
    fn test_pin_text_truncates_long_excerpt() {
        let long = "x".repeat(2000);
        let text = pin_text("idle", &long);
        assert!(text.len() < 900);
        assert!(text.starts_with("🟢 idle\n\n"));
    }
}
