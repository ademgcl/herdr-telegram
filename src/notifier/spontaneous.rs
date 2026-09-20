//! Spontaneous answer pushes (forum + DM): the body alone, delivery-gated.
//! Split from `cards` (300-line file limit).
use crate::{
    herdr::client::list_panes,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{chunks, emoji},
};

pub(crate) use super::spontaneous_gate::{arm_superseded, part_blocked};

/// Per-owner completion for DM multi-owner pushes: complete when at
/// least one owner received every part. All-or-nothing across ALL
/// owners reposts to healthy owners up to 3× (15s apart) when one
/// owner blocks the bot, plus cross-settle repeats (no `last_done`
/// stamp) — one blocked owner must not spam the rest. Pure for tests.
pub(crate) fn dm_complete(per_owner_landed: &[usize], parts: usize) -> bool {
    parts > 0 && per_owner_landed.iter().any(|&n| n >= parts)
}

/// Liveness verdict before any topic sync/post: a dead pane must never
/// re-mint its topic (resurrection) nor buzz post-cancel. Fail-closed:
/// an ambiguous read (Err/empty) mints/posts nothing — the next tick
/// retries. Single source for `post_spontaneous_card` + `settle_check`.
/// Pure for tests. `live`: `None` = RPC Err, `Some(vec)` = Ok list.
#[derive(Debug, PartialEq)]
pub(crate) enum Liveness {
    Allow,
    Dead,
    Ambiguous,
}

pub(crate) fn liveness(live: Option<&Vec<String>>, pane: &str) -> Liveness {
    match live {
        None => Liveness::Ambiguous,
        Some(l) if l.is_empty() => Liveness::Ambiguous,
        Some(l) if l.iter().any(|p| p == pane) => Liveness::Allow,
        Some(_) => Liveness::Dead,
    }
}

/// Answer push: the body alone (never a status-word lead), plus the reply
/// affordance when input is needed. Blocked keeps its urgent prefix.
/// True when a part landed: drops must neither stamp `last_done` (it
/// would suppress the next settle) nor consume the caller's baseline.
/// `armed_at`: settle instant (Some) or None (DM immediate) — re-checked
/// AFTER the sync RPC, right before the first send (window = send only).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn post_spontaneous_card(
    s: &AppState,
    pane: &str,
    kind: &str,
    space: &str,
    settled: &str,
    body: &str,
    armed_at: Option<std::time::Instant>,
    spaces_ok: bool,
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
        // Liveness before sync (fail-closed verdict above): never
        // re-mint a dead pane's topic; ambiguous reads post nothing.
        if liveness(list_panes(&s.cfg.socket).await.ok().as_ref(), pane) != Liveness::Allow {
            return false;
        }
        // Pruned (human-deleted) topics retire the dialog — including
        // a delete raced create (thread None), where nothing posts yet.
        // Skipped on degraded spaces: never mint a stub on outage.
        if !spaces_ok {
            return false;
        }
        let (thread_opt, pruned) = s.topics.sync_topic_prune(pane, kind, space).await;
        if pruned {
            crate::handlers::dialog::retire_dialog(s, pane).await;
        }
        if let Some(thread) = thread_opt {
            // Inside-post re-check: a job/final landing during the sync
            // above owns the reply now — send nothing (window is now the
            // send RPC only, no sync between check and send). A /cancel
            // or newer arm in the same window aborts too.
            if s.job_live(pane).await {
                return false;
            }
            if let Some(at) = armed_at {
                let cur = s.debounce.lock().await.get(pane).cloned();
                if arm_superseded(cur.as_ref().map(|(st, a)| (st.as_str(), a)), settled, &at) {
                    return false;
                }
            }
            if let Some(at) = armed_at
                && let Some(t) = s.last_done.lock().await.get(pane)
                && *t > at
            {
                return false;
            }
            // Moved-on stays silent (§3, forum parity with the DM
            // branch below + settle_check pre-post/retry guards):
            // PC-side work starting during the sync above owns the
            // pane — a flip to `working` after the observe must not
            // buzz the stale settle.
            if super::retry_guard::moved_on(
                s.status.lock().await.get(pane).map(String::as_str),
                settled,
            ) {
                return false;
            }
            for part in &parts {
                // Per-part retarget (finalize parity): a submit landing
                // during a slow multi-part flood-wait must stop the stale
                // tail — the pre-loop check alone still posts part 2+
                // beside the new prompt's turn.
                if part_blocked(s, pane, settled, armed_at, false).await {
                    return false;
                }
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
        // Liveness first (forum parity): a pane dying after the
        // settle pre-check must not buzz post-cancel; ambiguous reads
        // post nothing (next tick retries).
        if liveness(list_panes(&s.cfg.socket).await.ok().as_ref(), pane) != Liveness::Allow {
            return false;
        }
        // No debounce arm is ever inserted in DM mode, so currency is
        // arm-optional (`dm_arm_blocks`): a missing arm proceeds, a
        // present-but-mismatched arm still aborts. Same verdict as the
        // per-part loop (single source) — Moved-on stays silent (§3).
        if part_blocked(s, pane, settled, armed_at, true).await {
            return false;
        }
        let mut per_owner = vec![0usize; s.cfg.owners.len()];
        // Stale-tail stop (forum parity above): break out and fall
        // through to the complete+stamp logic — an owner completed
        // before the supersede still stamps, the rest retry next tick.
        let mut superseded = false;
        for (oi, id) in s.cfg.owners.iter().enumerate() {
            for part in &parts {
                // Same per-part retarget as the forum branch above (DM
                // currency: missing arm proceeds, present-but-stale aborts).
                if part_blocked(s, pane, settled, armed_at, true).await {
                    superseded = true;
                    break;
                }
                let mid = s.tg.send_msg(*id, None, part, None).await;
                if let Some(m) = mid {
                    per_owner[oi] += 1;
                    if settled == "done" {
                        let _ = s.tg.set_reaction(*id, m, Some("✅")).await;
                    }
                }
                s.remember(*id, mid, pane).await;
            }
            if superseded {
                break;
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
#[path = "spontaneous_tests.rs"]
mod tests;
