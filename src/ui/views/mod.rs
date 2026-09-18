use super::emoji::{emoji, worst_status};
use crate::types::{AgentDetail, AgentRow, MAX_MSG_UNITS, WorkspaceInfo};

#[cfg(test)]
mod tests;

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
    if !cur.is_empty() || out.is_empty() {
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
        text.push_str(&format!(
            "{emo} #{} {} — {} agent(s)\n",
            s.number,
            s.label,
            mine.len()
        ));
    }

    text.push_str("\n🤖 agents\n");
    if agents.is_empty() {
        text.push_str("(none — tap ➕ or wait for detection)\n");
    }
    for a in agents {
        text.push_str(&format!(
            "{} {} @ {} [{}]\n",
            emoji(&a.status),
            a.kind,
            ws_label(spaces, &a.ws),
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
        text.push_str(&format!(
            "{} {} [{}]\n   {title}\n",
            emoji(&a.status),
            a.kind,
            a.pane
        ));
    }
    text
}

pub fn build_agent_card_text(a: &AgentDetail) -> String {
    let branch_line = match &a.branch {
        Some(b) => format!("\nbranch: 🌿 {b}"),
        None => String::new(),
    };
    format!(
        "{} {} [{}]\nstatus: {}\nspace: {}{}\ncwd: {}\ntitle: {}",
        emoji(&a.status),
        a.kind,
        a.pane,
        a.status,
        a.ws,
        branch_line,
        a.cwd,
        a.title,
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
    let mut card = format!("📌 **{kind}** · `{pane}`\nWorkspace: `{space}`");
    if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
        card.push_str(&format!("\nTitle: {}", t.trim()));
    }
    if let Some(b) = branch.filter(|b| !b.trim().is_empty()) {
        card.push_str(&format!("\nBranch: 🌿 {}", b.trim()));
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

pub fn help_text() -> &'static str {
    "`/agents`   control panel: spaces, agents, ➕ spawn\n\
     `/spawn <kind> [space]`   spawn a new agent\n\
     `/space [name]`   new space + shell topic\n\
     `/model`    current model + free-Zen picker (opencode)\n\
     `/quit`     drop the agent to a shell (busy confirms)\n\
     `/kill`     close the pane completely (confirms first)\n\
     `/shell`    open a fresh shell pane\n\
     `/pane`     shell pane in this space, stays here\n\
     `/split`    sibling shell pane (longer side; or right|down)\n\
     `/read [n]`     recent output of focused agent (`/output [n]` too)\n\
     `/card`     re-post the question + buttons (never stuck)\n\
     `/esc`      guarded Esc: dismiss, blocked-only\n\
      `/status`   refresh agent status card\n\
      `/history [n]`  prompts you sent here (cross-device catch-up)\n\
      `/reset`    paced reset of all topics (re-sync from Herdr)\n\
      `/cancel [all|<pane>]` abort focused job(s), all = everything / exit keys-mode\n\
     `/keys <pane> y enter`   send raw keys\n\n\
     ↩️ reply to any bot message → talks to that agent\n\
     plain text → focused agent\n\n\
      alerts fire on 🛑 needs-input / 🏆 finish — just reply to them"
}

/// General-topic help (extracted from the General router so parity
/// tests pin it like every other surface): topic/DM-scoped commands
/// redirect here with guidance instead of 404ing.
pub fn general_help_text() -> &'static str {
    "🤖 **Herdr Telegram Bot**\n\n\
     • `/agents` — open spaces & agents control panel\n\
     • `/spawn <kind> [space]` — spawn a new agent & topic\n\
      • `/shell [space]` — open a fresh shell pane & topic\n\
      • `/pane [space]` — shell pane in this space, stays here\n\
      • `/space [name]` — new space + shell topic\n\
     • `/model` — inside an agent topic: model picker\n\
     • `/history [n]` — inside an agent topic: recent prompts\n\
     • `/card` `/esc` — inside an agent topic: fresh buttons / guarded dismiss\n\
     • `/quit` `/kill` `/split` `/read` `/output` `/status` `/keys` — inside the agent's topic (or DM)\n\
     • `/reset` — paced reset of all topics (re-sync from Herdr)\n\
      • `/cancel [all|<pane>]` — abort focused job(s), all = everything\n\n\
     💡 Each active agent has its own dedicated topic in this group! Switch to an agent's topic to chat with it directly."
}

pub fn topic_help_text(pane: &str, kind: &str) -> String {
    format!(
        "🤖 **{kind}** topic [{pane}]\n\n\
         • Plain text sends a prompt to this agent\n\
         • `/start` — show this help\n\
         • `/agents`, `/spawn` — control panel & spawn live in General/DM\n\
         • `/read [n]` or `/output [n]` — fetch recent terminal output\n\
         • `/history [n]` — recent prompts you sent here\n\
         • `/model` — current model + free-Zen picker (opencode)\n\
         • `/quit` — drop the agent to a shell (busy confirms)\n\
         • `/kill` — close this pane completely (confirms first)\n\
         • `/card` — re-post the question + buttons (never stuck)\n\
         • `/esc` — guarded Esc: dismiss, blocked-only\n\
          • `/shell [space]` — open a fresh shell pane\n\
          • `/pane [space]` — shell pane in this space, stays here\n\
          • `/space [name]` — new space + shell topic\n\
          • `/split` — sibling shell pane (longer side; or right|down)\n\
           • `/keys y enter` — send keystrokes\n\
          • `/cancel [all|<pane>]` — abort this pane (or scope), re-prompt to resume\n\
          • `/reset` — reset this topic (re-sync from Herdr)\n\
          • `/status` — refresh agent status card\n\
          • ✏️ rename this topic = renames in herdr (kept in sync)"
    )
}

/// Help for a shell-pane topic: the pane is a terminal now. Refuses
/// (`/quit`, `/card`, `/model`, `/shell`) are documented as behavior —
/// each has a router arm answering with guidance, never silence.
pub fn shell_help_text(pane: &str) -> String {
    format!(
        "💲 shell topic [{pane}]\n\n\
         • Plain text runs as a shell command\n\
         • `opencode`, `claude`, … — run one to re-enter as agent\n\
         • `/start` — show this help\n\
         • `/agents`, `/spawn` — control panel & spawn live in General/DM\n\
         • `/read [n]` — recent shell output (`/output [n]` too)\n\
         • `/history [n]` — recent shell commands\n\
         • `/esc` — send Esc (vim toggles mode)\n\
          • `/cancel [all|<pane>]` — abort this pane (or scope), re-prompt to resume\n\
          • `/space [name]` — new space + shell topic\n\
          • `/reset` — reset this topic (re-sync from Herdr)\n\
          • `/keys y enter` — send keystrokes\n\
          • `/kill` — close this pane completely (confirms first)\n\
         • `/pane [space]` — shell pane in this space, stays here\n\
         • `/split` — sibling shell pane (longer side; or right|down)\n\
         • `/status` — shell card\n\
         • `/quit` — already a shell (nothing to drop)\n\
         • `/card` — nothing to answer in a shell\n\
         • `/model` — no agent here (run one first)\n\
         • `/shell` — already in a shell topic (`/pane` for a second)\n\
         • ✏️ rename this topic = renames in herdr (kept in sync)"
    )
}
