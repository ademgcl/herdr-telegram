//! Debounced spontaneous pushes: a settle must hold before its answer
//! buzzes, so micro-settle flicker mid-task stays silent. Blocked (needs
//! input) bypasses the debounce in the caller and posts immediately.

use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    state::AppState,
    topics::names::display_status,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji, ws_label},
};
use std::time::{Duration, Instant};

/// A settle must hold this long before a spontaneous answer pushes —
/// micro-settle flicker mid-task stays on the icon instead of buzzing.
/// Blocked (needs input) always pushes immediately.
pub(crate) const SETTLE_DEBOUNCE_SECS: u64 = 15;

/// Debounced spontaneous push: posts the fresh reply only if this settle
/// is still current (no newer transition, no prompt takeover, no newer
/// card) after the grace period. Baseline is consumed either way.
pub(crate) async fn settle_check(s: AppState, pane: String, settled: String, armed_at: Instant) {
    tokio::time::sleep(Duration::from_secs(SETTLE_DEBOUNCE_SECS)).await;
    let current = s.debounce.lock().await.get(&pane).cloned();
    if current
        .map(|(st, at)| st != settled || at != armed_at)
        .unwrap_or(true)
    {
        return;
    }
    s.debounce.lock().await.remove(&pane);
    if s.jobs.lock().await.contains_key(&pane) {
        return;
    }
    if s.status
        .lock()
        .await
        .get(&pane)
        .map(|st| st != &settled)
        .unwrap_or(true)
    {
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
    let base = s.seen.lock().await.get(&pane).cloned().unwrap_or_default();
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let body = join_trimmed(&final_block(&source, ""));
    s.seen.lock().await.insert(pane.clone(), screen);
    let info = get_agent(&s.cfg.socket, &pane).await.ok();
    let (kind, ws_id) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone()),
        None => ("?".into(), "?".into()),
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    // Settle confirmed: icon follows now (even with no fresh body),
    // through the same done→idle decay as live observations.
    let settled_age = s
        .settled_at
        .lock()
        .await
        .get(&pane)
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    let display = display_status(&settled, settled_age);
    s.topics.sync_topic(&pane, &kind, raw_space, display).await;
    // Short genuine answers ("ok", "done") still push: the debounce
    // and delta above already gate noise, and a silent settle leaves
    // the owner unaware. Only a truly empty body stays quiet.
    if body.is_empty() {
        return;
    }
    post_spontaneous_card(&s, &pane, &kind, raw_space, &settled, &body).await;
}

/// Answer push: the body alone (never a status-word lead), plus the reply
/// affordance when input is needed. Blocked keeps its urgent prefix.
pub(crate) async fn post_spontaneous_card(
    s: &AppState,
    pane: &str,
    kind: &str,
    space: &str,
    settled: &str,
    body: &str,
) {
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
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());

    if let Some(forum) = s.cfg.forum {
        let settled_age = s
            .settled_at
            .lock()
            .await
            .get(pane)
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let display = display_status(settled, settled_age);
        if let Some(thread) = s.topics.sync_topic(pane, kind, space, display).await {
            for part in &parts {
                let mid = s.tg.send_msg(forum, Some(thread), part, None).await;
                s.remember(forum, mid, pane).await;
            }
        }
    } else {
        for id in &s.cfg.owners {
            for part in &parts {
                let mid = s.tg.send_msg(*id, None, part, None).await;
                s.remember(*id, mid, pane).await;
            }
        }
    }
}
