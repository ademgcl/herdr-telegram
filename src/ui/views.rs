use crate::types::{AgentDetail, AgentRow, WorkspaceInfo, MAX_MSG_UNITS};
use super::emoji::{emoji, worst_status};

pub fn fit(text: &str, max_units: usize) -> String {
    let total_units = text.encode_utf16().count();
    if total_units <= max_units {
        return text.to_string();
    }
    let marker = "\n\n… [truncated] …\n\n";
    let marker_units = marker.encode_utf16().count();
    if max_units <= marker_units {
        let mut out = String::new();
        for ch in text.chars() {
            if out.encode_utf16().count() + ch.len_utf16() > max_units {
                break;
            }
            out.push(ch);
        }
        return out;
    }
    let avail = max_units - marker_units;
    let head = avail * 60 / 100;
    let tail = avail - head;
    let mut h = String::new();
    for ch in text.chars() {
        if h.encode_utf16().count() + ch.len_utf16() > head {
            break;
        }
        h.push(ch);
    }
    let rest = &text[text.char_indices().nth(h.chars().count()).map(|(i, _)| i).unwrap_or(0)..];
    let mut t = String::new();
    for ch in rest.chars().rev() {
        if t.encode_utf16().count() + ch.len_utf16() > tail {
            break;
        }
        t.insert(0, ch);
    }
    format!("{h}{marker}{t}")
}

pub fn fit_msg(text: &str) -> String {
    fit(text, MAX_MSG_UNITS)
}

pub fn build_menu_text(spaces: &[WorkspaceInfo], agents: &[AgentRow]) -> String {
    let mut text = String::from("🗂 spaces\n");
    for s in spaces {
        let mine: Vec<&str> = agents
            .iter()
            .filter(|a| a.ws == s.id)
            .map(|a| a.status.as_str())
            .collect();
        let emo = worst_status(mine.clone());
        text.push_str(&format!("{emo} #{} {} — {} agent(s)\n", s.number, s.label, mine.len()));
    }

    text.push_str("\n🤖 agents\n");
    if agents.is_empty() {
        text.push_str("(none — tap ➕ or wait for detection)\n");
    }
    for a in agents {
        let sp_label = spaces
            .iter()
            .find(|s| s.id == a.ws)
            .map(|s| s.label.as_str())
            .unwrap_or(&a.ws);
        text.push_str(&format!(
            "{} {} @ {} [{}]\n",
            emoji(&a.status),
            a.kind,
            sp_label,
            a.pane,
        ));
    }
    text
}

pub fn build_ws_text(ws: &str, spaces: &[WorkspaceInfo], agents: &[AgentRow]) -> String {
    let label = spaces
        .iter()
        .find(|s| s.id == ws)
        .map(|s| format!("#{} {}", s.number, s.label))
        .unwrap_or_else(|| ws.to_string());

    let mut text = format!("🖥 {label}\n\n");
    if agents.is_empty() {
        text.push_str("(no live agents here)\n");
    }
    for a in agents {
        let title: String = a.title.chars().take(36).collect();
        text.push_str(&format!("{} {} [{}]\n   {title}\n", emoji(&a.status), a.kind, a.pane));
    }
    text
}

pub fn build_agent_card_text(a: &AgentDetail) -> String {
    format!(
        "{} {} [{}]\nstatus: {}\nspace: {}\ncwd: {}\ntitle: {}",
        emoji(&a.status),
        a.kind,
        a.pane,
        a.status,
        a.ws,
        a.cwd,
        a.title,
    )
}

pub fn agents_summary(spaces: &[WorkspaceInfo], agents: &[AgentRow], highlight: &str) -> String {
    let label = |id: &str| {
        spaces
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut out = String::new();
    for r in agents {
        let mark = if r.pane == highlight { "▶️" } else { "·" };
        out.push_str(&format!(
            "{mark}{} {} @ {}\n",
            emoji(&r.status),
            r.kind,
            label(&r.ws),
        ));
    }
    out
}

pub fn help_text() -> &'static str {
    "/agents   control panel: spaces, agents, ➕ spawn\n\
     /read     recent output of focused agent\n\
     /cancel   abort prompts / exit keys-mode\n\
     /keys <pane> y enter   send raw keys\n\n\
     ↩️ reply to any bot message → talks to that agent\n\
     plain text → focused agent\n\n\
     alerts fire on ⛔ needs-input / ✅ finish — just reply to them"
}

pub fn topic_help_text(pane: &str, kind: &str) -> String {
    format!(
        "🤖 **{kind}** topic [{pane}]\n\n\
         • Plain text sends a prompt to this agent\n\
         • `/read` or `/output` — fetch recent terminal output\n\
         • `/keys y enter` — send keystrokes\n\
         • `/cancel` — abort running prompt\n\
         • `/status` — refresh agent status card"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fit_short_text() {
        let short = "Hello World";
        assert_eq!(fit(short, 100), short);
    }

    #[test]
    fn test_fit_truncation() {
        let long = "A".repeat(100);
        let fitted = fit(&long, 40);
        assert!(fitted.contains("… [truncated] …"));
        assert!(fitted.encode_utf16().count() <= 40);
    }

    #[test]
    fn test_topic_help_text() {
        let help = topic_help_text("w1:p1", "claude");
        assert!(help.contains("claude"));
        assert!(help.contains("w1:p1"));
    }
}
