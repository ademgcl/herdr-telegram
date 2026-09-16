//! Debounced spontaneous pushes: a settle must hold before its answer
//! buzzes, so micro-settle flicker mid-task stays silent. Blocked (needs
//! input) bypasses the debounce in the caller and posts immediately.

use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji, ws_label},
};
use std::time::{Duration, Instant};

/// A settle must hold this long before a spontaneous answer pushes —
/// micro-settle flicker mid-task stays silent instead of buzzing.
/// Blocked (needs input) always pushes immediately.
pub(crate) const SETTLE_DEBOUNCE_SECS: u64 = 15;

/// Debounced spontaneous push: posts the fresh reply only if this settle
/// is still current (no newer transition, no prompt takeover, no newer
/// card) after the grace period. Baseline anchors on delivery AND on
/// stray/empty (else the same stray re-RPCs every settle forever).
pub(crate) async fn settle_check(s: AppState, pane: String, settled: String, armed_at: Instant) {
    tokio::time::sleep(Duration::from_secs(SETTLE_DEBOUNCE_SECS)).await;
    // No spontaneous cards during reset: threads are dying/respawning,
    // posting a card would burn the 429 budget. Baseline is not consumed —
    // the first tick after reset re-sees the delta.
    if crate::handlers::reset::is_resetting() {
        return;
    }
    {
        // Single guard: a newer arm inserted between a separate get and
        // remove would be deleted with the stale one (dropped answer).
        let mut db = s.debounce.lock().await;
        let cur = db.get(&pane).cloned();
        if cur
            .as_ref()
            .map(|(st, at)| st != &settled || at != &armed_at)
            .unwrap_or(true)
        {
            return;
        }
        db.remove(&pane);
    }
    if s.jobs.lock().await.contains_key(&pane) {
        return;
    }
    // Settle holds across idle↔done sampling: a fast done→idle collapses
    // (no re-arm), so the done-armed check must still fire on idle —
    // else the fresh delta rots and no card ever posts. Blocked still
    // needs exact match (dialog turnover below re-arms its own checks).
    let settled_ok = s
        .status
        .lock()
        .await
        .get(&pane)
        .map(|st| {
            st == &settled
                || (matches!(settled.as_str(), "idle" | "done")
                    && matches!(st.as_str(), "idle" | "done"))
        })
        .unwrap_or(false);
    if !settled_ok {
        return;
    }
    if s.last_done
        .lock()
        .await
        .get(&pane)
        .map(|t| *t > armed_at)
        .unwrap_or(false)
    {
        return;
    }
    let screen: Vec<String> = read_agent_output(&s.cfg.socket, &pane, 80)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    // Post-read re-check: a prompt that landed during the RPCs above owns
    // the pane now — the watcher's final card covers it, never us too.
    // Same for a final that stamped last_done while we were reading.
    if s.jobs.lock().await.contains_key(&pane) {
        return;
    }
    if s.last_done
        .lock()
        .await
        .get(&pane)
        .map(|t| *t > armed_at)
        .unwrap_or(false)
    {
        return;
    }
    // Outage/unknown (Err collapsed above): never anchor an empty
    // screen — it would wipe a good baseline and repost full scrollback
    // as fresh on the next settle.
    if screen.is_empty() {
        return;
    }
    let base = s.seen.lock().await.get(&pane).cloned().unwrap_or_default();
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let body = join_trimmed(&final_block(&source, ""));
    // Baseline anchors on delivery; stray/empty also anchors (same-screen
    // strays must not re-RPC every settle). Drops (topic race, outage)
    // leave the delta for the next tick — see post_spontaneous_card.
    // Reset started mid-debounce: threads are dying — never sync/post
    // into the migration (the re-check at wake is 15s+ of RPCs old).
    if crate::handlers::reset::is_resetting() {
        return;
    }
    let info = get_agent(&s.cfg.socket, &pane).await.ok();
    let (kind, ws_id) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone()),
        None => ("?".into(), "?".into()),
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    s.topics.sync_topic(&pane, &kind, raw_space).await;
    // Single stray chars (picker echoes, vim residue) never page; real
    // shorts ("ok", "done") do. Empty stays silent — but the baseline
    // still advances so the stray doesn't haunt every future settle.
    if body.chars().count() < 2 {
        s.seen.lock().await.insert(pane.clone(), screen);
        return;
    }
    // Pre-post re-check (narrows the check→send window to just the send
    // RPC): a job/final that landed during get_agent/spaces/sync above
    // owns the reply now — never double-post with the watcher.
    if s.jobs.lock().await.contains_key(&pane) {
        return;
    }
    if s.last_done
        .lock()
        .await
        .get(&pane)
        .map(|t| *t > armed_at)
        .unwrap_or(false)
    {
        return;
    }
    // Bounded retry on SEND outage only: a blip exactly at settle must
    // not eat a one-shot reply no future transition would surface. Two
    // extra tries, then the next transition owns it as before. Refusals
    // (job takeover, newer last_done, newer arm, status moved on) break
    // instead of spinning 30s on a stale screen; reset aborts the loop.
    let mut delivered = false;
    for _ in 0..3 {
        if crate::handlers::reset::is_resetting() {
            break;
        }
        if post_spontaneous_card(&s, &pane, &kind, raw_space, &settled, &body, Some(armed_at)).await
        {
            delivered = true;
            break;
        }
        if s.jobs.lock().await.contains_key(&pane) {
            break;
        }
        if s.last_done
            .lock()
            .await
            .get(&pane)
            .map(|t| *t > armed_at)
            .unwrap_or(false)
        {
            break;
        }
        // A newer arm owns the reply now (stale body must not beat it).
        if s.debounce.lock().await.contains_key(&pane) {
            break;
        }
        // Spontaneous new work started mid-retry: down-window silence.
        let moved_on = s
            .status
            .lock()
            .await
            .get(&pane)
            .map(|st| {
                st != &settled
                    && !(matches!(settled.as_str(), "idle" | "done")
                        && matches!(st.as_str(), "idle" | "done"))
            })
            .unwrap_or(true);
        if moved_on {
            break;
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
    if delivered {
        s.seen.lock().await.insert(pane.clone(), screen);
    }
}

/// Answer push: the body alone (never a status-word lead), plus the reply
/// affordance when input is needed. Blocked keeps its urgent prefix.
/// Returns true when at least one part was delivered: drops (topic race,
/// Telegram outage) must neither stamp `last_done` (it would suppress the
/// next settle) nor consume the caller's baseline.
/// `armed_at`: settle debounce instant (Some) or None (DM immediate) —
/// re-checked AFTER the sync RPC, immediately before the first send, so
/// the check→send window is the send RPC only (no sync RPC between).
pub(crate) async fn post_spontaneous_card(
    s: &AppState,
    pane: &str,
    kind: &str,
    space: &str,
    settled: &str,
    body: &str,
    armed_at: Option<std::time::Instant>,
) -> bool {
    // NOTE: deliberately NOT touching focus here — background pushes must
    // never hijack where the owner's next plain-text message gets delivered.
    let text = match settled {
        "blocked" => format!(
            "{} {body}\n↩️ reply or type in topic to answer",
            emoji("blocked")
        ),
        _ => body.to_string(),
    };
    let parts = chunks(&text, MAX_MSG_UNITS);
    println!(
        "[alert] spontaneous card {pane}: {} part(s), body {} chars",
        parts.len(),
        body.len()
    );

    let mut delivered = false;
    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.sync_topic(pane, kind, space).await {
            // Inside-post re-check: a job/final landing during the sync
            // above owns the reply now — send nothing (window is now the
            // send RPC only, no sync between check and send).
            if s.jobs.lock().await.contains_key(pane) {
                return false;
            }
            if let Some(at) = armed_at
                && let Some(t) = s.last_done.lock().await.get(pane)
                && *t > at
            {
                return false;
            }
            for part in &parts {
                let mid = s.tg.send_msg(forum, Some(thread), part, None).await;
                if let Some(m) = mid {
                    delivered = true;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(forum, m, Some("✅")).await;
                    }
                }
                s.remember(forum, mid, pane).await;
            }
        }
    } else {
        // DM immediate: no sync RPC, check immediately before sends.
        if s.jobs.lock().await.contains_key(pane) {
            return false;
        }
        if let Some(at) = armed_at
            && let Some(t) = s.last_done.lock().await.get(pane)
            && *t > at
        {
            return false;
        }
        for id in &s.cfg.owners {
            for part in &parts {
                let mid = s.tg.send_msg(*id, None, part, None).await;
                if let Some(m) = mid {
                    delivered = true;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(*id, m, Some("✅")).await;
                    }
                }
                s.remember(*id, mid, pane).await;
            }
        }
    }
    if delivered {
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
    }
    delivered
}
