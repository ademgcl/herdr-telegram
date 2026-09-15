use super::{emoji, views::ws_label, worst_status};
use crate::types::{AgentRow, WorkspaceInfo};
use serde_json::{Value, json};

pub const SPAWN_KINDS: &[&str] = &[
    "opencode", "claude", "codex", "gemini", "cursor", "copilot", "amp", "droid", "grok", "qwen",
];

pub fn btn(text: impl Into<String>, data: &str) -> Value {
    json!({"text": text.into(), "callback_data": data})
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
                    btn(format!("{emo} {}", s.label), &format!("w:{}", s.id))
                })
                .collect(),
        );
    }

    kb.push(vec![btn("➕ spawn agent", "n"), btn("🆕 new space", "N")]);

    for a in agents {
        kb.push(vec![btn(
            format!(
                "{} {} @ {}",
                emoji(&a.status),
                a.kind,
                ws_label(spaces, &a.ws)
            ),
            &format!("a:{}", a.pane),
        )]);
    }

    json!(kb)
}

pub fn workspace_kb(ws: &str, agents: &[AgentRow]) -> Value {
    let mut kb = Vec::new();
    for a in agents {
        let title: String = a.title.chars().take(28).collect();
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

pub fn agent_card_kb(pane: &str, ws: &str) -> Value {
    json!([
        [
            btn("📄 output", &format!("o:{pane}")),
            btn("⌨️ keys", &format!("K:{pane}")),
        ],
        [
            btn("🤖 model", &format!("M:list:{pane}")),
            btn("🔄 refresh", &format!("a:{pane}")),
        ],
        [btn(format!("← {ws}"), &format!("w:{ws}")),],
    ])
}

pub fn pane_output_kb(pane: &str) -> Value {
    json!([[btn("🔄 refresh", &format!("p:{pane}"))]])
}
