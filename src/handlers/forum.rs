use crate::state::AppState;
use serde_json::Value;

/// Strip a `@BotName` mention suffix from a command (`/model@MyBot` → `/model`).
/// Group clients append it when only one bot is present; plain commands pass through.
pub(crate) fn bare_cmd(cmd: &str) -> &str {
    cmd.split('@').next().unwrap_or(cmd)
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
    if text.is_empty() {
        return;
    }

    // `/space [name]` works everywhere (General + any agent/shell topic):
    // new space + shell topic to keep chatting in. Blank auto-labels.
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    if bare_cmd(raw_cmd) == "/space" {
        let label = if super::space::check_label(arg) {
            arg.to_string()
        } else {
            super::space::next_label(&s).await
        };
        super::space::open_space(&s, chat, thread_id, &label).await;
        return;
    }

    // If inside an agent topic thread:
    if let Some(th) = thread_id
        && let Some(pane) = s.topics.pane_of_thread(th)
    {
        if let Some(mid) = msg["message_id"].as_i64() {
            s.topics.record_msg(&pane, mid);
        }
        println!(
            "[forum] topic msg for {pane}: {}",
            text.chars().take(40).collect::<String>()
        );
        super::forum_topic::handle_topic_agent_message(s, chat, th, &pane, text).await;
        return;
    }

    // Inside General topic or non-agent thread:
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
}
