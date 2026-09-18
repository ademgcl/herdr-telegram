//! Debounced spontaneous pushes: a settle must hold before its answer
//! buzzes (micro-settle flicker stays silent; blocked posts immediately).

use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji, ws_label},
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// A settle must hold this long before a spontaneous answer pushes —
/// micro-settle flicker mid-task stays silent instead of buzzing.
/// Blocked (needs input) always pushes immediately.
pub(crate) const SETTLE_DEBOUNCE_SECS: u64 = 15;

/// Pure reset-arm consume (testable without the 15s debounce sleep): a
/// stale arm left armed would abort the post-reset retry — consume only
/// the exact arm, never a newer one.
pub(crate) fn consume_reset_arm(
    db: &mut HashMap<String, (String, Instant)>,
    pane: &str,
    armed_at: Instant,
) {
    if db.get(pane).map(|(_, at)| at == &armed_at).unwrap_or(false) {
        db.remove(pane);
    }
}

/// Debounced spontaneous push: posts the fresh reply only if this settle
/// is still current after the grace period. Baselines anchor on delivery
/// AND on stray/empty (else the same stray re-RPCs every settle forever).
pub(crate) async fn settle_check(s: AppState, pane: String, settled: String, armed_at: Instant) {
    tokio::time::sleep(Duration::from_secs(SETTLE_DEBOUNCE_SECS)).await;
    // No spontaneous cards during reset (threads dying; 429 budget).
    // Baseline unconsumed (next tick re-sees the delta); the arm IS
    // consumed (see consume_reset_arm): stale would abort the retry.
    if crate::handlers::reset::is_resetting() {
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        return;
    }
    {
        // Single guard: a newer arm must not be deleted with the stale
        // one (dropped answer).
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
    // Settle holds across idle↔done sampling: a fast done→idle collapses,
    // so the done-armed check still fires on idle (blocked needs exact).
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
    // Post-read re-check: a prompt/final that landed during the read
    // owns the pane now — never double-post with the watcher.
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
    // screen — it would wipe a good baseline and repost scrollback.
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
    // strays must not re-RPC every settle). Drops leave the delta.
    // Reset mid-debounce: never sync/post into the migration.
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
    // shorts ("ok", "done") do. Empty stays silent but advances the
    // baseline so the stray doesn't haunt future settles.
    if body.chars().count() < 2 {
        s.seen.lock().await.insert(pane.clone(), screen);
        return;
    }
    // Pre-post re-check (window = send RPC only): a job/final that
    // landed during get_agent/spaces/sync owns the reply now.
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
    // Bounded retry on SEND outage only (a blip must not eat a one-shot
    // reply). Two extra tries, then the next transition owns it.
    // Refusals (takeover, newer last_done/arm, moved-on) break instead
    // of spinning; reset aborts the loop.
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
/// True when a part landed: drops must neither stamp `last_done` (it
/// would suppress the next settle) nor consume the caller's baseline.
/// `armed_at`: settle instant (Some) or None (DM immediate) — re-checked
/// AFTER the sync RPC, right before the first send (window = send only).
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

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
