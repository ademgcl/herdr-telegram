//! Spontaneous answer pushes (forum + DM): the body alone, delivery-gated.
//! Split from `cards` (300-line file limit).
use crate::{
    herdr::client::list_panes,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji},
};

/// Per-owner completion for DM multi-owner pushes: complete when at
/// least one owner received every part. All-or-nothing across ALL
/// owners reposts to healthy owners up to 3× (15s apart) when one
/// owner blocks the bot, plus cross-settle repeats (no `last_done`
/// stamp) — one blocked owner must not spam the rest. Pure for tests.
pub(crate) fn dm_complete(per_owner_landed: &[usize], parts: usize) -> bool {
    parts > 0 && per_owner_landed.iter().any(|&n| n >= parts)
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
    // DM mode has no topics: the reply hint names the DM, not a topic.
    let text = match settled {
        "blocked" if s.cfg.forum.is_some() => format!(
            "{} {body}\n↩️ reply or type in topic to answer",
            emoji("blocked")
        ),
        "blocked" => format!("{} {body}\n↩️ reply here to answer", emoji("blocked")),
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
        // Pruned (human-deleted) topics retire the dialog — including
        // a delete raced create (thread None), where nothing posts yet.
        let (thread_opt, pruned) = s.topics.sync_topic_prune(pane, kind, space).await;
        if pruned {
            crate::handlers::dialog::retire_dialog(s, pane).await;
        }
        if let Some(thread) = thread_opt {
            // Inside-post re-check: a job/final landing during the sync
            // above owns the reply now — send nothing (window is now the
            // send RPC only, no sync between check and send). A /cancel
            // or newer arm in the same window aborts too.
            if s.jobs.lock().await.contains_key(pane) {
                return false;
            }
            if let Some(at) = armed_at
                && s.debounce
                    .lock()
                    .await
                    .get(pane)
                    .map(|(st, a)| st != settled || a != &at)
                    .unwrap_or(true)
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
        let mut per_owner = vec![0usize; s.cfg.owners.len()];
        for (oi, id) in s.cfg.owners.iter().enumerate() {
            for part in &parts {
                let mid = s.tg.send_msg(*id, None, part, None).await;
                if let Some(m) = mid {
                    per_owner[oi] += 1;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(*id, m, Some("✅")).await;
                    }
                }
                s.remember(*id, mid, pane).await;
            }
        }
        // Per-owner (not product): one blocked owner must not hold the
        // healthy ones hostage for 3 retries + every future settle.
        let complete = dm_complete(&per_owner, parts.len());
        if complete {
            s.last_done
                .lock()
                .await
                .insert(pane.to_string(), std::time::Instant::now());
        }
        return complete;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dm_complete_per_owner() {
        // One healthy owner completes despite a blocked one — no
        // retry-spam to the healthy, no cross-settle repeats.
        assert!(dm_complete(&[2, 0], 2));
        assert!(dm_complete(&[1], 1));
        assert!(dm_complete(&[2, 2], 2));
        // Nobody whole: partials must not stamp (retry reposts full).
        assert!(!dm_complete(&[1, 1], 2));
        assert!(!dm_complete(&[1, 0], 2));
        assert!(!dm_complete(&[], 1));
        assert!(!dm_complete(&[0], 0));
        assert!(!dm_complete(&[], 0));
    }
}
