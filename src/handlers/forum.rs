use serde_json::Value;
use crate::{
    herdr::client::{
        create_workspace, get_agent, list_agents, list_workspaces,
        read_agent_output, send_agent_keys, spawn_agent,
    },
    jobs::enqueue_prompt,
    state::AppState,
    ui::{
        agent_card_kb, build_agent_card_text, build_menu_text,
        main_menu_kb, topic_help_text, ws_label,
    },
};

/// Strip a `@BotName` mention suffix from a command (`/model@MyBot` → `/model`).
/// Group clients append it when only one bot is present; plain commands pass through.
pub(crate) fn bare_cmd(cmd: &str) -> &str {
    cmd.split('@').next().unwrap_or(cmd)
}

pub async fn handle_forum_message(s: AppState, chat: i64, msg: &Value) {    let from = msg["from"]["id"].as_i64().unwrap_or(0);
    if !s.cfg.owners.contains(&from) {
        return;
    }

    let thread_id = msg["message_thread_id"].as_i64();
    let text = msg["text"].as_str().unwrap_or("").trim();
    if text.is_empty() { return; }

    // If inside an agent topic thread:
    if let Some(th) = thread_id {
        if let Some(pane) = s.topics.pane_of_thread(th) {
            println!("[forum] topic msg for {pane}: {}", text.chars().take(40).collect::<String>());
            handle_topic_agent_message(s, chat, th, &pane, text).await;
            return;
        }
    }

    // Inside General topic or non-agent thread:
    handle_general_forum_message(s, chat, thread_id, text).await;
}

async fn handle_topic_agent_message(
    s: AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    text: &str,
) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    // Group clients may send `/cmd@BotName` — strip the mention suffix.
    let cmd = bare_cmd(raw_cmd);

    let Ok(agent) = get_agent(&s.cfg.socket, pane).await else {
        // No agent in this pane: shell CLI mode (or a lingering dead pane,
        // which the shell side reports as gone).
        super::shell::handle_shell_topic(s, chat, thread_id, pane, text).await;
        return;
    };
    println!("[forum] got agent {} cmd={arg:?}...", agent.pane, arg = &text.chars().take(30).collect::<String>());


    if cmd == "/help" {
        s.tg.send_msg(chat, Some(thread_id), &topic_help_text(pane, &agent.kind), None).await;
        return;
    }

    if cmd == "/cancel" {
        s.typewait.lock().await.remove(&chat);
        let count = s.cancel_all_jobs().await;
        s.tg.send_msg(chat, Some(thread_id), &format!("✋ cancelled {count} prompt(s)"), None).await;
        return;
    }

    if cmd == "/quit" {
        super::shell::quit_to_shell(&s, chat, Some(thread_id), pane).await;
        return;
    }

    // Answering a waiting prompt (set by the ⌨️ button on blocked cards).
    if let Some(wpane) = s.typewait.lock().await.remove(&chat) {
        match super::interactive::type_text(&s, &wpane, text).await {
            Ok(()) => { s.tg.send_msg(chat, Some(thread_id), &format!("⌨️ typed into {wpane} + ⏎"), None).await; }
            Err(e) => { s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ type failed: {e}"), None).await; }
        }
        return;
    }

    if cmd == "/read" || cmd == "/output" {
        let lines = arg.parse::<u32>().unwrap_or(80);
        match read_agent_output(&s.cfg.socket, pane, lines).await {
            Ok(out) => {
                let body = if out.is_empty() { "(no output)".into() } else { out };
                s.tg.send_msg(chat, Some(thread_id), &body, None).await;
            }
            Err(e) => { s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None).await; }
        }
        return;
    }

    if cmd == "/keys" {
        if arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), "usage: `/keys y enter`", None).await;
            return;
        }
        let keys: Vec<&str> = arg.split_whitespace().collect();
        match send_agent_keys(&s.cfg.socket, pane, &keys).await {
            Ok(_) => { s.tg.send_msg(chat, Some(thread_id), "⌨️ keys sent", None).await; }
            Err(e) => { s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None).await; }
        }
        return;
    }

    if cmd == "/status" {
        let text = build_agent_card_text(&agent);
        let kb = agent_card_kb(pane, &agent.ws);
        s.tg.send_msg(chat, Some(thread_id), &text, Some(kb)).await;
        return;
    }

    if cmd == "/model" {
        if arg.is_empty() {
            super::model::show_model(&s, chat, Some(thread_id), pane).await;
        } else {
            let filter = super::model::search_filter(arg);
            super::model::switch_by_filter(&s, chat, Some(thread_id), pane, &filter, arg).await;
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, Some(thread_id), "unknown topic command. Type `/help` for available commands.", None).await;
        return;
    }

    // Bare message inside agent's topic -> prompt that agent!
    // Blocked panes reject text prompts ("requires interactive input"),
    // so type straight into the waiting prompt instead — no dead job.
    if agent.status == "blocked" {
        s.set_focus(pane).await;
        match super::interactive::type_text(&s, pane, text).await {
            Ok(()) => { s.tg.send_msg(chat, Some(thread_id), &format!("⌨️ typed into {pane} + ⏎"), None).await; }
            Err(_) => { super::interactive::send_blocked_card(&s, chat, Some(thread_id), pane).await; }
        }
        return;
    }
    s.set_focus(pane).await;
    enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
}

async fn handle_general_forum_message(
    s: AppState,
    chat: i64,
    thread_id: Option<i64>,
    text: &str,
) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = bare_cmd(raw_cmd);

    if cmd == "/start" || cmd == "/help" {
        let msg = "🤖 **Herdr Telegram Bot**\n\n\
                   • `/agents` — open spaces & agents control panel\n\
                   • `/spawn <kind> [workspace]` — spawn a new agent & topic\n\
                   • `/newspace <name>` — create a new workspace\n\
                   • `/model` — inside an agent topic: model picker\n\
                   • `/cancel` — abort pending jobs\n\n\
                   💡 Each active agent has its own dedicated topic in this group! Switch to an agent's topic to chat with it directly.";
        s.tg.send_msg(chat, thread_id, msg, None).await;
        return;
    }

    if cmd == "/agents" {
        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
        s.tg.send_msg(chat, thread_id, &build_menu_text(&spaces, &agents), Some(main_menu_kb(&spaces, &agents))).await;
        return;
    }

    if cmd == "/spawn" {
        let (kind, ws) = match arg.split_once(char::is_whitespace) {
            Some((k, w)) => (k, Some(w)),
            None if !arg.is_empty() => (arg, None),
            _ => {
                s.tg.send_msg(chat, thread_id, "usage: `/spawn <kind> [space]` (e.g. `/spawn opencode space-1`)", None).await;
                return;
            }
        };
        s.tg.send_msg(chat, thread_id, &format!("⏳ spawning {kind}…"), None).await;
        match spawn_agent(&s.cfg.socket, kind, ws).await {
            Ok(row) => {
                let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                let space = ws_label(&spaces, &row.ws);
                if let Some(topic_th) = s.topics.sync_topic(&row.pane, &row.kind, space, &row.status).await {
                    s.tg.send_msg(chat, thread_id, &format!("✅ Started {} [{}] in topic #{topic_th}", row.kind, row.pane), None).await;
                } else {
                    s.tg.send_msg(chat, thread_id, &format!("✅ Started {} [{}]", row.kind, row.pane), None).await;
                }
            }
            Err(e) => {
                s.tg.send_msg(chat, thread_id, &format!("⚠️ spawn failed: {e}"), None).await;
            }
        }
        return;
    }

    if cmd == "/newspace" && !arg.is_empty() {
        match create_workspace(&s.cfg.socket, arg).await {
            Ok(id) => { s.tg.send_msg(chat, thread_id, &format!("✅ created workspace `{arg}` ({id})"), None).await; }
            Err(e) => { s.tg.send_msg(chat, thread_id, &format!("⚠️ failed: {e}"), None).await; }
        }
        return;
    }

    if cmd == "/cancel" {
        let count = s.cancel_all_jobs().await;
        s.tg.send_msg(chat, thread_id, &format!("✋ cancelled {count} pending job(s)"), None).await;
        return;
    }

    if cmd == "/model" {
        s.tg.send_msg(chat, thread_id, "open an agent's topic and run `/model` there — each topic is one agent.", None).await;
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, thread_id, "unknown command — see `/help` or `/agents`", None).await;
        return;
    }

    // Bare text in General topic
    s.tg.send_msg(
        chat,
        thread_id,
        "💡 To talk to an agent, please open its dedicated topic or use `/agents` to spawn one.",
        None,
    ).await;
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
}
