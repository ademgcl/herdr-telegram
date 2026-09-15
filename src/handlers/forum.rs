use serde_json::Value;
use crate::{
    herdr::client::{
        get_agent, read_agent_output, send_agent_keys,
    },
    jobs::enqueue_prompt,
    state::AppState,
    ui::{
        agent_card_kb, build_agent_card_text, topic_help_text,
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
    let text = msg["text"].as_str().or_else(|| msg["caption"].as_str()).unwrap_or("").trim();
    if text.is_empty() { return; }

    // `/space [name]` works everywhere (General + any agent/shell topic):
    // new space + shell topic to keep chatting in. Blank auto-labels.
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    if bare_cmd(raw_cmd) == "/space" {
        let label = if super::space::check_label(arg) { arg.to_string() } else { super::space::next_label(&s).await };
        super::space::open_space(&s, chat, thread_id, &label).await;
        return;
    }

    // If inside an agent topic thread:
    if let Some(th) = thread_id {
        if let Some(pane) = s.topics.pane_of_thread(th) {
            println!("[forum] topic msg for {pane}: {}", text.chars().take(40).collect::<String>());
            handle_topic_agent_message(s, chat, th, &pane, text).await;
            return;
        }
    }

    // Inside General topic or non-agent thread:
    super::general::handle_general_forum_message(s, chat, thread_id, text).await;
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
        super::shell_topic::handle_shell_topic(s, chat, thread_id, pane, text).await;
        return;
    };
    println!("[forum] got agent {} cmd={arg:?}...", agent.pane, arg = &text.chars().take(30).collect::<String>());


    if cmd == "/help" {
        s.tg.send_msg(chat, Some(thread_id), &topic_help_text(pane, &agent.kind), None).await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, Some(thread_id)));
        s.runwait.lock().await.remove(&(chat, Some(thread_id)));
        s.typewait.lock().await.remove(&(chat, Some(thread_id)));
        let n = s.cancel_jobs_for(pane).await;
        let msg = if n { "cancelled pane job(s)" } else { "nothing running" };
        s.tg.send_msg(chat, Some(thread_id), msg, None).await;
        return;
    }
    if super::tap::consume_runkey(&s, chat, Some(thread_id), text).await { return; }

    if cmd == "/quit" {
        super::shell::quit_to_shell(&s, chat, Some(thread_id), pane).await;
        return;
    }

    if cmd == "/kill" {
        super::kill::ask_kill(&s, chat, Some(thread_id), pane).await;
        return;
    }

    if cmd == "/split" {
        let dir = match arg { "" | "right" => "right", "down" => "down", _ => {
            s.tg.send_msg(chat, Some(thread_id), "usage: `/split [right|down]`", None).await; return; } };
        super::shell::open_split(&s, chat, Some(thread_id), pane, dir).await;
        return;
    }

    if cmd == "/shell" {
        // No arg: shell next to this agent (same workspace).
        let ws = if arg.is_empty() { agent.ws.as_str() } else { arg };
        super::shell::open_shell(&s, chat, Some(thread_id), Some(ws)).await;
        return;
    }

    // Answering a waiting prompt (set by the ⌨️ button on blocked cards).
    if let Some(wpane) = s.typewait.lock().await.remove(&(chat, Some(thread_id))) {
        match super::tap::type_text(&s, &wpane, text).await {
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
        match super::tap::type_text(&s, pane, text).await {
            Ok(()) => { s.tg.send_msg(chat, Some(thread_id), &format!("⌨️ typed into {pane} + ⏎"), None).await; }
            Err(_) => { super::dialog::send_blocked_card(&s, chat, Some(thread_id), pane).await; }
        }
        return;
    }
    s.set_focus(pane).await;
    enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
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
