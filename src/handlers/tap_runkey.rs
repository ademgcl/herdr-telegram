//! Armed run/key waiter consume (split from `tap_input`, 300-line
//! file limit): runwait runs text as a shell command, keywait sends it
//! as keys to the pane.
use crate::{
    herdr::client::{get_agent, send_agent_keys, send_pane_keys},
    state::AppState,
};

/// Consume an armed run/key waiter for (chat, thread): runwait runs the
/// text as a shell command in-topic, keywait sends it as keys to the pane
/// (agent keys when it holds an agent, pane keys otherwise).
pub async fn consume_runkey(s: &AppState, chat: i64, thread: Option<i64>, text: &str) -> bool {
    let key = super::forum::waiter_key(chat, thread);
    if let Some((ws, at)) = s.runwait.lock().await.remove(&key) {
        // Fail-closed expiry at consume (not just the 60s hygiene tick):
        // a corpse waiter firing arbitrarily later would execute stale
        // input as a shell command. Swallowed with a notice, never run.
        if crate::state::guard::claim_stale(
            at,
            std::time::Instant::now(),
            crate::state::guard::RUNWAIT_STALE_SECS,
        ) {
            s.tg.send_msg(chat, thread, crate::ui::ARM_EXPIRED, None)
                .await;
            return true;
        }
        super::shell::handle_run_command(s, chat, thread, &ws, text).await;
        return true;
    }
    // Upfront consume (runwait parity): a /cancel landing between the
    // peek and the keys send must win — get-then-remove sent keys after
    // a cancel. Stay-armed paths below re-insert with the ORIGINAL
    // instant (never re-stamp now, which would immortalize the waiter).
    if let Some((pane, at)) = s.keywait.lock().await.remove(&key) {
        // Same corpse bound for keys: stale keys executing into a live
        // session is the dangerous half of waiter staleness.
        if crate::state::guard::claim_stale(
            at,
            std::time::Instant::now(),
            crate::state::guard::KEYWAIT_STALE_SECS,
        ) {
            s.tg.send_msg(chat, thread, crate::ui::ARM_EXPIRED, None)
                .await;
            return true;
        }
        // Never interleave with an owned key sequence (mirrors /keys):
        // a tap answer or model switch in flight owns the pane's input
        // until it lands. The waiter re-arms — the retry is just
        // sending the message again (a stale corpse self-evicts here).
        if s.block_held(&pane).await || s.model_held(&pane).await {
            s.keywait.lock().await.insert(key, (pane, at));
            s.tg.send_msg(chat, thread, crate::ui::TAP_MODEL_IN_FLIGHT, None)
                .await;
            return true;
        }
        // Bounded input (single source with every /keys arm): refuse
        // empty/over-cap, never silently truncate a partial write.
        let keys = match super::shell_validate::validate_keys_text(text) {
            Ok(k) => k,
            Err(msg) => {
                s.tg.send_msg(chat, thread, &msg, None).await;
                return true;
            }
        };
        // Classify without guessing: get_agent-ok means an agent owns
        // the pane (agent keys); a confirmed live pane with no agent is
        // a shell (pane keys). Anything unreadable refuses visibly —
        // never sends blind. A selective get_agent blip on an agent
        // pane must not route one batch as pane keys (dm_info parity:
        // only confirmed death reads as shell) — re-arm with the
        // original instant and refuse, never consume on ambiguity.
        let was_shell = match get_agent(&s.cfg.socket, &pane).await {
            Ok(_) => false,
            Err(e) if crate::herdr::rpc::should_retry_agent_lookup(&e.to_string()) => {
                s.keywait.lock().await.insert(key, (pane, at));
                s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return true;
            }
            Err(_) => {
                // Distinguish gone (shell or dead) from blip: a failed
                // pane list must not read as anything — refuse visibly.
                match crate::herdr::client::list_panes(&s.cfg.socket).await {
                    Ok(l) if l.contains(&pane.to_string()) => true,
                    Ok(_) => {
                        // Dead pane: waiter re-arms (pre-upfront-remove
                        // parity) — the corpse bound above evicts it, and
                        // consuming here would silently reroute the next
                        // message instead of repeating the visible error.
                        s.keywait.lock().await.insert(key, (pane, at));
                        s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_TARGET, None)
                            .await;
                        return true;
                    }
                    Err(_) => {
                        // Double outage: fail-closed — re-arm with the
                        // original instant, refuse visibly, never consume
                        // on an ambiguous read. Normalized key (the
                        // upfront consume above): a raw re-insert would
                        // fork a second entry beside the original.
                        s.keywait.lock().await.insert(key, (pane, at));
                        s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                            .await;
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
                s.tg.send_msg(chat, thread, crate::ui::KEYS_SENT, None)
                    .await;
            }
            Err(e) => {
                s.tg.send_msg(
                    chat,
                    thread,
                    &format!(
                        "{}: {}",
                        crate::ui::KEYS_FAILED_PC,
                        crate::types::mask_home(&e.to_string())
                    ),
                    None,
                )
                .await;
            }
        }
        return true;
    }
    false
}
