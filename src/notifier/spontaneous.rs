//! Spontaneous answer pushes (forum + DM): the body alone, delivery-gated.
//! Split from `cards` (300-line file limit).
use crate::{
    herdr::client::list_panes,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji},
};

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

    let mut landed = 0usize;
    let mut expected = 0usize;
    if let Some(forum) = s.cfg.forum {
        // Liveness before sync: never re-mint a dead pane's topic.
        // Fail-open on Err/empty (a herdr blip must not eat replies).
        if let Ok(live) = list_panes(&s.cfg.socket).await
            && !live.is_empty()
            && !live.iter().any(|p| p == pane)
        {
            return false;
        }
        if let Some(thread) = s.topics.sync_topic(pane, kind, space).await {
            // Inside-post re-check: a job/final landing during the sync
            // above owns the reply now — send nothing (window is now the
            // send RPC only, no sync between check and send). A /cancel
            // or newer arm in the same window aborts too.
            if s.jobs.lock().await.contains_key(pane) {
                return false;
            }
            if let Some(at) = armed_at
                && s.debounce.lock().await.get(pane).map(|(st, a)| st != settled || a != &at).unwrap_or(true)
            {
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
                    landed += 1;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(forum, m, Some("✅")).await;
                    }
                }
                s.remember(forum, mid, pane).await;
            }
            expected = parts.len();
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
                    landed += 1;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(*id, m, Some("✅")).await;
                    }
                }
                s.remember(*id, mid, pane).await;
            }
        }
        expected = parts.len() * s.cfg.owners.len();
    }
    // All-or-nothing: a partial multi-part push must not stamp
    // last_done (it would suppress the next settle) — the retry
    // reposts the full body. Single-part keeps the old any-landed
    // semantics via the same count check.
    let complete = expected > 0 && landed >= expected;
    if complete {
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
    }
    complete
}
