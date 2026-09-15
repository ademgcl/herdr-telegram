use super::shell_common::shell_card_text;
use crate::{
    herdr::client::{get_agent, list_panes, send_agent_keys},
    state::AppState,
};
use tokio::time::{Duration, sleep};

/// Drop the pane's agent to a shell. Idle/done only: quitting a working
/// or blocked agent would destroy real work or answer a dialog blindly.
pub async fn quit_to_shell(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let agent = match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => a,
        Err(_) => {
            // Dead pane (topic lingering) vs live shell — only the latter
            // gets the shell card. Fail-open: a failed list call proceeds
            // to the shell card (later commands fail visibly if truly dead).
            let dead = list_panes(&s.cfg.socket)
                .await
                .map(|l| !l.contains(&pane.to_string()))
                .unwrap_or(false);
            if dead {
                s.tg.send_msg(chat, thread, &format!("⚠️ pane {pane} is gone"), None)
                    .await;
                return;
            }
            let mid =
                s.tg.send_msg(chat, thread, &shell_card_text(pane), None)
                    .await;
            s.remember(chat, mid, pane).await;
            s.set_focus(pane).await;
            return;
        }
    };
    if !matches!(agent.status.as_str(), "idle" | "done") {
        s.tg.send_msg(
            chat,
            thread,
            &format!(
                "⛔ {} is {} — wait for idle (never quit live work)",
                pane, agent.status
            ),
            None,
        )
        .await;
        return;
    }
    // Waiters are NOT cleared here: a failed quit (keys, still-busy)
    // must leave them armed for the retry. Success clears via
    // cancel_jobs_for → clear_waiters below.
    if send_agent_keys(&s.cfg.socket, pane, &["ctrl+c"])
        .await
        .is_err()
    {
        s.tg.send_msg(chat, thread, "⚠️ quit keys failed — quit on the PC", None)
            .await;
        return;
    }
    // Confirm herdr sees a shell (agent_not_found) before claiming it.
    let mut shelled = false;
    for _ in 0..4 {
        sleep(Duration::from_millis(1500)).await;
        if get_agent(&s.cfg.socket, pane).await.is_err() {
            shelled = true;
            break;
        }
    }
    if !shelled {
        s.tg.send_msg(
            chat,
            thread,
            &format!("⚠️ still in {} — try again or quit on the PC", agent.kind),
            None,
        )
        .await;
        return;
    }
    s.cancel_jobs_for(pane).await;
    s.clear_pane(pane).await;
    // Badge shell NOW (not on the next watchdog cycle) so a fast
    // re-enter still hushes correctly in the notifier.
    s.status
        .lock()
        .await
        .insert(pane.to_string(), "shell".to_string());
    s.topics.mark_shell(pane).await;
    let mid =
        s.tg.send_msg(chat, thread, &shell_card_text(pane), None)
            .await;
    s.remember(chat, mid, pane).await;
    s.set_focus(pane).await;
}
