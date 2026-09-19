use super::tap_classify::dialog_stalled;
use crate::{
    handlers::dialog::{parse_options, refresh_blocked_card, winner_lines},
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
            crate::ui::ANSWER_IN_FLIGHT.into(),
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
    // 1:1 with the card (which hides Type on option dialogs): free text
    // has nowhere to land there — typed keys + Enter can confirm the
    // wrong highlight. Refuse before sending anything; a stale waiter
    // racing a turnover fails here and the next message retries fresh.
    if !parse_options(&winner_lines(&before)).is_empty() {
        return Err(TypeError::Failed(
            "that question takes an option — tap a button, or /card for fresh buttons".into(),
        ));
    }
    // Strip before the slow send (cards rule: strip before slow RPC):
    // buttons stay live across the 1.5s send otherwise, and a double
    // type contends silent on blockop — text untouched, markup-only.
    crate::handlers::dialog::strip_tracked(s, pane).await;
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
                "text sent but the question didn't advance — tap a button instead, or answer on the PC".into(),
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
    // Landed: settle the answered cards now (buttons off) — the heal
    // above reconciles after. Tracking + signature stay: a turnover
    // reposts, a resume resolves.
    crate::handlers::dialog::strip_tracked(s, pane).await;
    // Landed: remember what the owner sent (typed answers are history
    // too — Resumed callers route through enqueue instead, no double).
    s.push_history(pane, text).await;
    Ok(())
}

/// Arm the typed-answer waiter (B:type button): the next message in
/// this chat types into the pane's waiting prompt + Enter. Split from
/// `tap_answer` (300-line file limit).
pub(crate) async fn arm_type_waiter(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    // 1:1 arming: a stale Type button racing a turnover to options
    // must not arm a waiter that eats the next message into refuses
    // (nor wipe the sibling run/key waiters below). Unreadable
    // screen arms anyway — outage must not brick typing; the
    // send-path gate backstops stale arms.
    let screen = read_screen_visible(&s.cfg.socket, pane, 30).await;
    if !screen.is_empty()
        && !parse_options(&winner_lines(&screen)).is_empty()
    {
        s.tg.send_msg(
            chat,
            thread,
            "that question takes an option — tap a button:",
            None,
        )
        .await;
        super::escape::handle_card_topic(s, chat, thread, pane).await;
        return;
    }
    // Refuse BEFORE mutating anything: an armed shell command in the
    // sibling waiters is live work — dropping it for a refused arm
    // loses the command and misroutes the next message as a prompt.
    // Same-pane re-arms refresh the instant and proceed.
    let occupant = s.typewait.lock().await.get(&(chat, thread)).map(|(p, _)| p.clone());
    if let Some(other) = occupant
        && other != pane
    {
        s.tg.send_msg(
            chat,
            thread,
            &format!(
                "an answer is already armed for {other} — /cancel it first, then tap Type again"
            ),
            None,
        )
        .await;
        return;
    }
    // Exclusive waiter: drop sibling run/key waiters for this key so
    // the next message types instead of running.
    s.runwait.lock().await.remove(&(chat, thread));
    s.keywait.lock().await.remove(&(chat, thread));
    s.typewait.lock().await.insert(
        (chat, thread),
        (pane.to_string(), std::time::Instant::now()),
    );
    let mid = s
        .tg
        .send_msg(
            chat,
            thread,
            "⌨️ type your answer as the next message (⏎ sends it)",
            None,
        )
        .await;
    s.remember(chat, mid, pane).await;
}

/// Consume an armed run/key waiter for (chat, thread): runwait runs the
/// text as a shell command in-topic, keywait sends it as keys to the pane
/// (agent keys when it holds an agent, pane keys otherwise).
pub async fn consume_runkey(s: &AppState, chat: i64, thread: Option<i64>, text: &str) -> bool {
    if let Some((ws, at)) = s.runwait.lock().await.remove(&(chat, thread)) {
        // Fail-closed expiry at consume (not just the 60s hygiene tick):
        // a corpse waiter firing arbitrarily later would execute stale
        // input as a shell command. Swallowed with a notice, never run.
        if crate::state::guard::claim_stale(
            at,
            std::time::Instant::now(),
            crate::state::guard::RUNWAIT_STALE_SECS,
        ) {
            s.tg.send_msg(chat, thread, crate::ui::ARM_EXPIRED, None).await;
            return true;
        }
        super::shell::handle_run_command(s, chat, thread, &ws, text).await;
        return true;
    }
    if let Some((pane, at)) = s.keywait.lock().await.get(&(chat, thread)).cloned() {
        // Same corpse bound for keys: stale keys executing into a live
        // session is the dangerous half of waiter staleness.
        if crate::state::guard::claim_stale(
            at,
            std::time::Instant::now(),
            crate::state::guard::KEYWAIT_STALE_SECS,
        ) {
            s.keywait.lock().await.remove(&(chat, thread));
            s.tg.send_msg(chat, thread, crate::ui::ARM_EXPIRED, None).await;
            return true;
        }
        // Never interleave with an owned key sequence (mirrors /keys):
        // a tap answer or model switch in flight owns the pane's input
        // until it lands. The waiter stays armed — the retry is just
        // sending the message again (a stale corpse self-evicts here).
        if s.block_held(&pane).await || s.model_held(&pane).await {
            s.tg.send_msg(chat, thread, crate::ui::TAP_MODEL_IN_FLIGHT, None)
                .await;
            return true;
        }
        s.keywait.lock().await.remove(&(chat, thread));
        // Bounded input (single source with every /keys arm): refuse
        // empty/over-cap, never silently truncate a partial write.
        let keys: Vec<&str> = text.split_whitespace().collect();
        if let Err(msg) = super::shell_validate::validate_keys_len(keys.len()) {
            s.tg.send_msg(chat, thread, &msg, None).await;
            return true;
        }
        // Classify without guessing: get_agent-ok means an agent owns
        // the pane (agent keys); a confirmed live pane with no agent is
        // a shell (pane keys). Anything unreadable refuses visibly —
        // never sends blind. (A selective get_agent blip on an agent
        // pane routes one batch as pane keys; the keys land in the same
        // pane, and the waiter is consumed, so the blast radius is one
        // mistyped batch, never cross-pane injection.)
        let was_shell = match get_agent(&s.cfg.socket, &pane).await {
            Ok(_) => false,
            Err(_) => {
                // Distinguish gone (shell or dead) from blip: a failed
                // pane list must not read as anything — refuse visibly.
                match crate::herdr::client::list_panes(&s.cfg.socket).await {
                    Ok(l) if l.contains(&pane.to_string()) => true,
                    Ok(_) => {
                        s.tg.send_msg(chat, thread, &format!("pane {pane} is gone"), None).await;
                        return true;
                    }
                    Err(_) => {
                        s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None).await;
                        return true;
                    }
                }
            }
        };
        let r = if !was_shell {
            send_agent_keys(&s.cfg.socket, &pane, &keys).await
        } else {
            send_pane_keys(&s.cfg.socket, &pane, &keys).await
        };
        match r {
            Ok(_) => {
                // Shell keys may have launched an agent — instant re-icon.
                if was_shell {
                    super::shell_lifecycle::spawn_flip_watch(s, &pane);
                }
                // Focus follows success (never armed — see handle_keys_arm).
                s.set_focus(&pane).await;
                s.tg.send_msg(chat, thread, crate::ui::KEYS_SENT, None).await;
            }
            Err(e) => {
                s.tg.send_msg(chat, thread, &format!("keys failed: {}", crate::types::mask_home(&e.to_string())), None)
                    .await;
            }
        }
        return true;
    }
    false
}
