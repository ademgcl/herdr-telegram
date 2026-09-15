use super::tap_classify::dialog_stalled;
use crate::{
    handlers::dialog::refresh_blocked_card,
    herdr::client::{
        get_agent, read_screen_visible, send_agent_keys, send_pane_input, send_pane_keys,
    },
    state::AppState,
};
use std::time::Duration;

/// Type free text into the waiting prompt (y/n answers, picker filters,
/// text inputs) + Enter, atomically: split text/Enter round-trips get
/// lost on redraw-heavy TUIs. Verified like button taps — a lying
/// "typed" ack is worse than none. The answer may advance to a SECOND
/// dialog with no status change, so re-check shortly and surface fresh
/// buttons.
pub async fn type_text(s: &AppState, pane: &str, text: &str) -> Result<(), String> {
    let socket = &s.cfg.socket;
    let before = read_screen_visible(socket, pane, 30).await;
    // Own the card through send + verify: a same-`blocked` observation
    // mid-sleep must not post a duplicate card or race the baseline.
    // Single-flight like button taps: concurrent types interleave.
    if !s.blockop.lock().await.insert(pane.to_string()) {
        return Err("answer already in flight — wait a beat".into());
    }
    let r = send_pane_input(socket, pane, text)
        .await
        .map_err(|e| e.to_string());
    if r.is_err() {
        s.blockop.lock().await.remove(pane);
        return r.map(|_| ());
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    if get_agent(socket, pane)
        .await
        .map(|a| a.status == "blocked")
        .unwrap_or(true)
    {
        let after = read_screen_visible(socket, pane, 30).await;
        if dialog_stalled(&before, &after) {
            s.blockop.lock().await.remove(pane);
            return Err("text sent but the dialog didn't advance — tap a button instead, or answer on the PC".into());
        }
    }
    s.blockop.lock().await.remove(pane);
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        refresh_blocked_card(&s2, &pane2).await;
    });
    Ok(())
}

/// Consume an armed run/key waiter for (chat, thread): runwait runs the
/// text as a shell command in-topic, keywait sends it as keys to the pane
/// (agent keys when it holds an agent, pane keys otherwise).
pub async fn consume_runkey(s: &AppState, chat: i64, thread: Option<i64>, text: &str) -> bool {
    if let Some(ws) = s.runwait.lock().await.remove(&(chat, thread)) {
        super::shell::handle_run_command(s, chat, thread, &ws, text).await;
        return true;
    }
    if let Some(pane) = s.keywait.lock().await.remove(&(chat, thread)) {
        let keys: Vec<&str> = text.split_whitespace().collect();
        let r = if get_agent(&s.cfg.socket, &pane).await.is_ok() {
            send_agent_keys(&s.cfg.socket, &pane, &keys).await
        } else {
            send_pane_keys(&s.cfg.socket, &pane, &keys).await
        };
        match r {
            Ok(_) => {
                s.tg.send_msg(chat, thread, "keys sent", None).await;
            }
            Err(e) => {
                s.tg.send_msg(chat, thread, &format!("keys failed: {e}"), None)
                    .await;
            }
        }
        return true;
    }
    false
}
