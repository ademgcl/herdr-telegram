use super::emoji::{emoji, worst_status};
use crate::types::{AgentDetail, AgentRow, MAX_MSG_UNITS, WorkspaceInfo};

mod help;
#[cfg(test)]
#[path = "mask_tests.rs"]
mod mask_tests;
#[cfg(test)]
#[path = "parity_tests.rs"]
mod parity_tests;
#[cfg(test)]
mod tests;
pub use help::*;

pub fn fit(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_string();
    }
    let marker = "\n\n… [truncated] …\n\n";
    if max_units <= marker.encode_utf16().count() {
        return text.chars().fold(String::new(), |mut out, ch| {
            if out.encode_utf16().count() + ch.len_utf16() <= max_units {
                out.push(ch);
            }
            out
        });
    }
    let avail = max_units - marker.encode_utf16().count();
    let (head, tail) = (avail * 60 / 100, avail - avail * 60 / 100);
    let mut units = 0;
    let mut head_end = 0;
    for (i, ch) in text.char_indices() {
        if units + ch.len_utf16() > head {
            break;
        }
        units += ch.len_utf16();
        head_end = i + ch.len_utf8();
    }
    let mut units = 0;
    let mut tail_start = text.len();
    for (i, ch) in text.char_indices().rev() {
        if units + ch.len_utf16() > tail {
            break;
        }
        units += ch.len_utf16();
        tail_start = i;
    }
    format!("{}{marker}{}", &text[..head_end], &text[tail_start..])
}

pub fn ws_label<'a>(spaces: &'a [WorkspaceInfo], id: &'a str) -> &'a str {
    spaces
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.label.as_str().trim())
        .filter(|l| !l.is_empty())
        .unwrap_or(id)
}

pub fn fit_msg(text: &str) -> String {
    fit(text, MAX_MSG_UNITS)
}

/// Split text into Telegram-sized chunks (line-aligned where possible).
pub fn chunks(text: &str, max_units: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_units = 0;
    for line in text.lines() {
        let line_units = line.encode_utf16().count();
        if line_units + 1 > max_units {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_units = 0;
            }
            out.push(fit(line, max_units));
            continue;
        }
        if cur_units > 0 && cur_units + 1 + line_units > max_units {
            out.push(std::mem::take(&mut cur));
            cur_units = 0;
        }
        if cur_units > 0 {
            cur.push('\n');
            cur_units += 1;
        }
        cur.push_str(line);
        cur_units += line_units;
    }
    // Newline-only bodies must not emit `[""]` (Telegram 400s it) —
    // only push a non-empty remainder (`out.is_empty()` would force an
    // empty chunk for `"\n"` / `"\n\n"`).
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn build_menu_text(spaces: &[WorkspaceInfo], agents: &[AgentRow]) -> String {
    let mut text = String::from("🗂 spaces\n");
    for s in spaces {
        let mine: Vec<&str> = agents
            .iter()
            .filter(|a| a.ws == s.id)
            .map(|a| a.status.as_str())
            .collect();
        let emo = worst_status(mine.iter().copied());
        // Masked (agent-card parity): space labels carry terminal paths
        // ($HOME/username) shown to the whole chat.
        let label = crate::types::mask_home(&s.label);
        text.push_str(&format!(
            "{emo} #{} {} — {} agent(s)\n",
            s.number,
            label,
            mine.len()
        ));
    }

    text.push_str("\n🤖 agents\n");
    if agents.is_empty() {
        text.push_str("(none — tap ➕ or wait for detection)\n");
    }
    for a in agents {
        let kind = crate::types::mask_home(&a.kind);
        let ws = crate::types::mask_home(ws_label(spaces, &a.ws));
        text.push_str(&format!(
            "{} {} @ {} [{}]\n",
            emoji(&a.status),
            kind,
            ws,
            a.pane,
        ));
    }
    text
}

pub fn build_ws_text(ws: &str, spaces: &[WorkspaceInfo], agents: &[AgentRow]) -> String {
    let label = spaces
        .iter()
        .find(|s| s.id == ws)
        .map(|s| format!("#{} {}", s.number, crate::types::mask_home(&s.label)))
        .unwrap_or_else(|| ws.to_string());

    let mut text = format!("🖥 {label}\n\n");
    if agents.is_empty() {
        text.push_str("(no live agents here)\n");
    }
    for a in agents {
        // Masked like the agent card below (titles carry terminal paths).
        let title: String = crate::types::mask_home(&a.title).chars().take(36).collect();
        text.push_str(&format!(
            "{} {} [{}]\n   {title}\n",
            emoji(&a.status),
            a.kind,
            a.pane
        ));
    }
    text
}

pub fn build_agent_card_text(a: &AgentDetail, space: &str) -> String {
    let branch_line = match &a.branch {
        Some(b) => format!("\nbranch: 🌿 {}", crate::types::mask_home(b)),
        None => String::new(),
    };
    // Mask $HOME in cwd + title + space (space labels carry terminal
    // paths — ws_text/menu parity, never raw $HOME to the chat).
    let cwd = crate::types::mask_home(&a.cwd);
    let title = crate::types::mask_home(&a.title);
    let space = crate::types::mask_home(space);
    format!(
        "{} {} [{}]\nstatus: {}\nspace: {}{}\ncwd: {}\ntitle: {}",
        emoji(&a.status),
        a.kind,
        a.pane,
        a.status,
        space,
        branch_line,
        cwd,
        title,
    )
}

/// Identity card for forum topics (F2 + D7): shows pane, workspace,
/// title (terminal/task), branch (if any), and live status.
pub fn build_identity_card_text(
    kind: &str,
    pane: &str,
    space: &str,
    status: &str,
    title: Option<&str>,
    branch: Option<&str>,
) -> String {
    // Plain text: no parse_mode is ever sent (build_send_msg_params
    // sends raw text), so Markdown would render literally.
    // Space masked: labels carry terminal paths ($HOME/username).
    let mut card = format!(
        "📌 {kind} · {pane}\nWorkspace: {}",
        crate::types::mask_home(space)
    );
    if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
        card.push_str(&format!("\nTitle: {}", crate::types::mask_home(t.trim())));
    }
    if let Some(b) = branch.filter(|b| !b.trim().is_empty()) {
        card.push_str(&format!(
            "\nBranch: 🌿 {}",
            crate::types::mask_home(b.trim())
        ));
    }
    let st_emoji = emoji(status);
    let display_status = if status == "idle" { "ready" } else { status };
    card.push_str(&format!("\nStatus: {st_emoji} {display_status}"));
    card
}

/// Last lines that fit within `max_units` — live progress tails read bottom-up.
pub fn tail_fit(lines: &[String], max_units: usize) -> String {
    let mut picked: Vec<&str> = Vec::new();
    let mut units = 0;
    for l in lines.iter().rev() {
        let lu = l.encode_utf16().count() + 1;
        if units + lu > max_units {
            break;
        }
        units += lu;
        picked.push(l);
    }
    picked.reverse();
    picked.join("\n").trim().to_string()
}
