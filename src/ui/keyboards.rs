use super::{emoji, views::ws_label, worst_status};
use crate::types::{AgentRow, WorkspaceInfo};
use serde_json::{Value, json};

pub const SPAWN_KINDS: &[&str] = &[
    "opencode", "claude", "codex", "gemini", "cursor", "copilot", "amp", "droid", "grok", "qwen",
];

pub fn btn(text: impl Into<String>, data: &str) -> Value {
    json!({"text": text.into(), "callback_data": data})
}

/// User-controlled label cap for BUTTON text: Telegram enforces ~64
/// BYTES (not chars) — 28 emoji × 4B = 112B still fails sendMessage
/// despite the char cap. Truncate to 28 chars, then pop whole chars
/// until the byte length fits.
pub(crate) fn btn_label(s: &str) -> String {
    let mut out: String = s.chars().take(28).collect();
    while out.len() > 64 {
        out.pop();
    }
    out
}

/// URL button (opens a link instead of sending a callback query).
/// Exactly one of `url` / `callback_data` per Bot API shape.
pub fn url_btn(text: impl Into<String>, url: &str) -> Value {
    json!({"text": text.into(), "url": url})
}

/// Deep link to a forum topic: `https://t.me/c/<id>/<thread>`.
/// Private supergroup ids look like `-1001234567890` → `1234567890`.
/// Returns None when no link can be built (DM mode, non-super ids).
pub fn topic_link(chat_id: i64, thread_id: i64) -> Option<String> {
    if thread_id <= 0 {
        return None;
    }
    let raw = chat_id.to_string();
    let inner = raw.strip_prefix("-100")?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("https://t.me/c/{inner}/{thread_id}"))
}

/// Single-button keyboard jumping straight into the pane's topic.
/// None when no link exists (caller sends the card without buttons).
pub fn open_topic_kb(chat_id: i64, thread_id: i64) -> Option<Value> {
    topic_link(chat_id, thread_id).map(|url| json!([[url_btn("➡️ open topic", &url)]]))
}

pub fn spawn_kb(ws: Option<&str>) -> Value {
    let mut rows: Vec<Vec<Value>> = SPAWN_KINDS
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|k| {
                    let data = match ws {
                        Some(w) => format!("k:{w}:{k}"),
                        None => format!("k:{k}"),
                    };
                    btn((*k).to_string(), &data)
                })
                .collect()
        })
        .collect();

    let back = match ws {
        Some(w) => format!("w:{w}"),
        None => "m".to_string(),
    };
    rows.push(vec![btn("← back", &back)]);
    json!(rows)
}

pub fn main_menu_kb(spaces: &[WorkspaceInfo], agents: &[AgentRow]) -> Value {
    let mut kb: Vec<Vec<Value>> = Vec::new();

    for pair in spaces.chunks(2) {
        kb.push(
            pair.iter()
                .map(|s| {
                    let mine: Vec<&str> = agents
                        .iter()
                        .filter(|a| a.ws == s.id)
                        .map(|a| a.status.as_str())
                        .collect();
                    let emo = worst_status(mine);
                    // Space labels are user-controlled (/space): cap them
                    // like titles below or long names fail sendMessage.
                    let label = btn_label(&s.label);
                    btn(format!("{emo} {label}"), &format!("w:{}", s.id))
                })
                .collect(),
        );
    }

    kb.push(vec![btn("➕ spawn agent", "n"), btn("🆕 new space", "N")]);

    for a in agents {
        // Space names are user-controlled: cap like titles, or long names
        // fail sendMessage with BUTTON_TEXT_INVALID.
        let ws = btn_label(ws_label(spaces, &a.ws));
        kb.push(vec![btn(
            format!("{} {} @ {}", emoji(&a.status), a.kind, ws),
            &format!("a:{}", a.pane),
        )]);
    }

    json!(kb)
}

pub fn workspace_kb(ws: &str, agents: &[AgentRow]) -> Value {
    let mut kb = Vec::new();
    for a in agents {
        let title = btn_label(&a.title);
        let label = if title.is_empty() {
            format!("{} {}", emoji(&a.status), a.kind)
        } else {
            format!("{} {} · {title}", emoji(&a.status), a.kind)
        };
        kb.push(vec![btn(label, &format!("a:{}", a.pane))]);
    }
    kb.push(vec![
        btn("➕ agent", &format!("n:{ws}")),
        btn("⌨️ run cmd", &format!("R:{ws}")),
    ]);
    kb.push(vec![btn("← spaces", "m")]);
    json!(kb)
}

pub fn agent_card_kb(pane: &str, ws_id: &str, ws_label: &str) -> Value {
    // Space labels are user-controlled (/space): cap like the menu arms
    // or long names fail sendMessage with BUTTON_TEXT_INVALID.
    let ws_label = btn_label(ws_label);
    json!([
        [
            btn("📄 output", &format!("o:{pane}")),
            btn("⌨️ keys", &format!("K:{pane}")),
        ],
        [
            btn("🤖 model", &format!("M:list:{pane}")),
            btn("🔄 refresh", &format!("a:{pane}")),
        ],
        [btn(format!("← {ws_label}"), &format!("w:{ws_id}")),],
    ])
}

pub fn pane_output_kb(pane: &str) -> Value {
    json!([[btn("🔄 refresh", &format!("p:{pane}"))]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_card_kb_caps_long_space_label() {
        let long = "x".repeat(100);
        let kb = agent_card_kb("w1:p1", "w1", &long);
        let text = kb[2][0]["text"].as_str().unwrap();
        assert!(text.chars().count() <= 30, "uncapped label: {text:?}");
    }

    #[test]
    fn test_btn_label_caps_bytes_not_just_chars() {
        // 28 emoji = 112B > 64B Telegram cap — must shrink by bytes.
        let emoji = "🔥".repeat(28);
        let out = super::btn_label(&emoji);
        assert!(out.len() <= 64, "byte overflow: {}B", out.len());
        assert!(out.chars().count() <= 28);
        // ASCII still caps at 28 chars.
        assert_eq!(super::btn_label(&"x".repeat(100)).chars().count(), 28);
    }

    #[test]
    fn test_topic_link_private_super() {
        assert_eq!(
            topic_link(-1001234567890, 3),
            Some("https://t.me/c/1234567890/3".to_string())
        );
    }

    #[test]
    fn test_topic_link_rejects() {
        assert_eq!(topic_link(-1001234567890, 0), None);
        assert_eq!(topic_link(-1001234567890, -2), None);
        assert_eq!(topic_link(12345, 3), None);
        assert_eq!(topic_link(-12345, 3), None);
        assert_eq!(topic_link(0, 1), None);
    }

    #[test]
    fn test_open_topic_kb_shape() {
        let kb = open_topic_kb(-1001234567890, 3).unwrap();
        assert_eq!(
            kb,
            json!([[ {"text": "➡️ open topic", "url": "https://t.me/c/1234567890/3"} ]])
        );
        assert!(open_topic_kb(12345, 3).is_none());
    }
}
