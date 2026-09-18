use super::tap_classify::dialog_stalled;
use crate::{
    handlers::dialog::refresh_blocked_card,
    herdr::client::{
        get_agent, read_screen_visible, send_agent_keys, send_pane_input, send_pane_keys,
    },
    state::AppState,
};
use std::time::Duration;

/// A typed answer that lost its race: the agent resumed between the
/// snapshot and the send, so the text must become a prompt instead of
/// input injected into live work.
/// Kept as a domain enum (not `Res`) on purpose: callers MATCH on
/// `Resumed` vs `Failed` to route (prompt vs error card) — boxing it
/// would erase the routing signal.
#[derive(Debug, PartialEq)]
pub enum TypeError {
    Resumed,
    Failed(String),
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeError::Resumed => write!(f, "agent resumed while answering"),
            TypeError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Type free text into the waiting prompt (y/n answers, picker filters,
/// text inputs) + Enter, atomically: split text/Enter round-trips get
/// lost on redraw-heavy TUIs. Verified like button taps — a lying
/// "typed" ack is worse than none. The answer may advance to a SECOND
/// dialog with no status change, so re-check shortly and surface fresh
/// buttons.
pub async fn type_text(s: &AppState, pane: &str, text: &str) -> Result<(), TypeError> {
    let socket = &s.cfg.socket;
    let before = read_screen_visible(socket, pane, 30).await;
    // Own the card through send + verify: a same-`blocked` observation
    // mid-sleep must not post a duplicate card or race the baseline.
    // Single-flight like button taps: concurrent types interleave.
    // RAII: cancellation mid-type must not wedge the pane.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        return Err(TypeError::Failed(
            "answer already in flight — wait a beat".into(),
        ));
    };
    // The agent may have resumed between the snapshot and now: typing
    // into live work injects the answer as stray input. Bail so the
    // caller routes the text as a prompt instead. Unreadable status is
    // fail-CLOSED (never inject blind): callers keep the waiter, so the
    // retry is just sending the message again after the blip.
    match get_agent(socket, pane).await {
        Err(_) => {
            return Err(TypeError::Failed("herdr status unreadable".into()));
        }
        Ok(a) if a.status != "blocked" => {
            return Err(TypeError::Resumed);
        }
        _ => {}
    }
    if let Err(e) = send_pane_input(socket, pane, text).await {
        return Err(TypeError::Failed(e.to_string()));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    if get_agent(socket, pane)
        .await
        .map(|a| a.status == "blocked")
        .unwrap_or(true)
    {
        let after = read_screen_visible(socket, pane, 30).await;
        if dialog_stalled(&before, &after) {
            return Err(TypeError::Failed(
                "text sent but the dialog didn't advance — tap a button instead, or answer on the PC".into(),
            ));
        }
        // Advanced: stamp NOTHING. The delayed refresh + watchdog post
        // the new dialog exactly once via claim + sig re-check; an
        // anchor here would blind them (stamp-only-on-delivery rule).
    }
    drop(_op);
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        refresh_blocked_card(&s2, &pane2).await;
    });
    // Landed: remember what the owner sent (typed answers are history
    // too — Resumed callers route through enqueue instead, no double).
    s.push_history(pane, text).await;
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
    if let Some(pane) = s.keywait.lock().await.get(&(chat, thread)).cloned() {
        // Never interleave with an owned key sequence (mirrors /keys):
        // a tap answer or model switch in flight owns the pane's input
        // until it lands. The waiter stays armed — the retry is just
        // sending the message again (a stale corpse self-evicts here).
        if s.block_held(&pane).await || s.model_held(&pane).await {
            s.tg.send_msg(chat, thread, "tap/model op in flight — send again in a beat", None)
                .await;
            return true;
        }
        s.keywait.lock().await.remove(&(chat, thread));
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
