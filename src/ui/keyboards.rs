use super::{emoji, views::ws_label, worst_status};
use crate::types::{AgentRow, WorkspaceInfo, mask_home};
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
                    // Space labels are user-controlled (/space): cap the
                    // FINAL text (prefix + label), never components — a
                    // capped label plus emoji still exceeds Telegram's 64B
                    // BUTTON_TEXT_INVALID.
                    // Masked (menu-text parity): space labels carry
                    // terminal paths ($HOME/username) — buttons ride to
                    // the whole chat like the text does.
                    let label = btn_label(&format!("{emo} {}", mask_home(&s.label)));
                    btn(label, &format!("w:{}", s.id))
                })
                .collect(),
        );
    }

    kb.push(vec![btn("➕ spawn agent", "n"), btn("🆕 new space", "N")]);

    for a in agents {
        // Space names are user-controlled: cap the FINAL text like the
        // menu arms above, or long names fail sendMessage with
        // BUTTON_TEXT_INVALID (kind is herdr-fed, capped the same way).
        // Masked (menu-text parity): kinds + space labels ride buttons
        // to the whole chat — never raw $HOME.
        let label = btn_label(&format!(
            "{} {} @ {}",
            emoji(&a.status),
            mask_home(&a.kind),
            mask_home(ws_label(spaces, &a.ws))
        ));
        kb.push(vec![btn(label, &format!("a:{}", a.pane))]);
    }

    json!(kb)
}

pub fn workspace_kb(ws: &str, agents: &[AgentRow]) -> Value {
    let mut kb = Vec::new();
    for a in agents {
        // Titles are user-controlled (terminal/task): cap the FINAL text
        // (emoji + kind + title), never components — capped parts plus a
        // raw kind still exceed Telegram's 64B BUTTON_TEXT_INVALID.
        // Masked (ws-text parity): titles carry terminal paths — never
        // raw $HOME on buttons.
        let label = if a.title.is_empty() {
            btn_label(&format!("{} {}", emoji(&a.status), mask_home(&a.kind)))
        } else {
            btn_label(&format!(
                "{} {} · {}",
                emoji(&a.status),
                mask_home(&a.kind),
                mask_home(&a.title)
            ))
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
    // Space labels are user-controlled (/space): cap the FINAL text —
    // the "← " prefix on an already-capped label still exceeds the cap.
    // Masked (agent-card-text parity): space labels carry terminal
    // paths — the back button rides to the chat like the card does.
    let back = btn_label(&format!("← {}", mask_home(ws_label)));
    json!([
        [
            btn("📄 output", &format!("o:{pane}")),
            btn("⌨️ keys", &format!("K:{pane}")),
        ],
        [
            btn("🤖 model", &format!("M:list:{pane}")),
            btn("🔄 refresh", &format!("a:{pane}")),
        ],
        [btn(back, &format!("w:{ws_id}")),],
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
    fn test_final_button_texts_fit_64_bytes() {
        // Adversarial user-controlled inputs: emoji-long space labels,
        // kinds and titles. The 64B cap applies to FINAL text (prefixes
        // included), or sendMessage fails with BUTTON_TEXT_INVALID.
        let spaces = vec![WorkspaceInfo {
            id: "w1".to_string(),
            label: "🔥".repeat(28),
            number: 1,
        }];
        let agents = vec![AgentRow {
            kind: "🔥".repeat(20),
            pane: "w1:p1".into(),
            title: "🔥".repeat(20),
            status: "working".into(),
            ws: "w1".into(),
        }];
        let kbs = vec![
            main_menu_kb(&spaces, &agents),
            workspace_kb("w1", &agents),
            agent_card_kb("w1:p1", "w1", &"🔥".repeat(28)),
        ];
        for kb in &kbs {
            for row in kb.as_array().unwrap() {
                for b in row.as_array().unwrap() {
                    let t = b["text"].as_str().unwrap();
                    assert!(t.len() <= 64, "button overflow ({}B): {t:?}", t.len());
                }
            }
        }
    }

    #[test]
    fn test_buttons_mask_home_paths() {
        // Button-text parity with menu/ws/agent-card texts: space
        // labels, kinds and titles carry terminal paths ($HOME/username)
        // shown to the whole chat — never raw $HOME on buttons.
        use crate::types::home_dir;
        let home = home_dir();
        assert!(!home.is_empty(), "test needs a HOME to mask");
        let spaces = vec![WorkspaceInfo {
            id: "w8".into(),
            label: format!("{home}/shop"),
            number: 8,
        }];
        let agents = vec![AgentRow {
            kind: format!("{home}/opencode"),
            pane: "w8:p1".into(),
            title: format!("{home}/proj title"),
            status: "working".into(),
            ws: "w8".into(),
        }];
        let kbs = vec![
            main_menu_kb(&spaces, &agents),
            workspace_kb("w8", &agents),
            agent_card_kb("w8:p1", "w8", &format!("{home}/shop")),
        ];
        for kb in &kbs {
            for row in kb.as_array().unwrap() {
                for b in row.as_array().unwrap() {
                    let t = b["text"].as_str().unwrap();
                    assert!(!t.contains(&*home), "button leaks $HOME: {t:?}");
                }
            }
        }
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
