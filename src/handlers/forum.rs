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
    // Waiter precedence: an armed typed/keyed/run answer for THIS thread
    // owns the next message (DM parity) — a literal "/space …" answer
    // must answer, never mint a billable workspace mid-dialog. Armed
    // threads (mapped or General) skip the mint and fall through to the
    // topic/General dispatch below, which consumes waiters first. (Waiters
    // are keyed (chat, thread) with thread always Some in forums, so a
    // thread-less update can never be armed — no None hole.)
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    if bare_cmd(raw_cmd) == "/space" {
        // Fail-closed: check (chat, thread_id) unconditionally — a
        // thread-less General update can still own a (chat, None) waiter
        // (panel taps carry no thread id), and minting is billable.
        let k = (chat, thread_id);
        let armed_here = s.typewait.lock().await.contains_key(&k)
            || s.keywait.lock().await.contains_key(&k)
            || s.runwait.lock().await.contains_key(&k);
        if !armed_here {
            let label = if super::space::check_label(arg) {
                arg.to_string()
            } else {
                super::space::next_label(&s).await
            };
            super::space::open_space(&s, chat, thread_id, &label).await;
            return;
        }
    }

    // If inside an agent topic thread:
    if let Some(th) = thread_id
        && let Some(pane) = s.topics.pane_of_thread(th)
    {
        if let Some(mid) = msg["message_id"].as_i64() {
            s.topics.record_msg(&pane, mid);
        }
        // Bodies stay out of the log (prompts can carry pasted
        // secrets); pane + length suffice for traffic forensics.
        println!(
            "[forum] topic msg for {pane} ({} chars)",
            text.chars().count()
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
