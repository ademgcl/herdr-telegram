//! Debounced spontaneous pushes: a settle must hold before its answer
//! buzzes (micro-settle flicker stays silent; blocked posts immediately).

use super::cards_retry::{SETTLE_DEBOUNCE_SECS, consume_reset_arm};
use super::spontaneous::post_spontaneous_card;
use crate::{
    herdr::client::{get_agent, list_panes, list_workspaces},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    state::AppState,
    ui::ws_label,
};
use std::time::{Duration, Instant};

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
        // one (dropped answer). The arm stays until post/abort so a
        // /cancel clearing debounce aborts before the first send —
        // removing early would make the cancel invisible and post once.
        let db = s.debounce.lock().await;
        let cur = db.get(&pane).cloned();
        if cur
            .as_ref()
            .map(|(st, at)| st != &settled || at != &armed_at)
            .unwrap_or(true)
        {
            return;
        }
    }
    if s.job_live(&pane).await {
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
    // Outage/unknown (empty on both sources): never anchor — it would
    // wipe a good baseline and repost scrollback. Bounded retry (split:
    // `cards_retry`): a one-shot return here loses the reply forever —
    // no new transition re-fires this arm.
    let mut screen: Vec<String> = super::cards_retry::read_screen_spontaneous(&s, &pane).await;
    if screen.is_empty() {
        let Some(retry) = super::cards_retry::read_screen_retry(&s, &pane, armed_at).await else {
            return;
        };
        screen = retry;
    }
    // Post-read re-check (both paths above): a prompt/final that landed
    // during the read owns the pane now — never double-post with the
    // watcher.
    if s.job_live(&pane).await {
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
    let base = s.seen.lock().await.get(&pane).cloned().unwrap_or_default();
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let body = join_trimmed(&final_block(&source, ""));
    // Baseline anchors on delivery; stray/empty also anchors (same-screen
    // strays must not re-RPC every settle). Drops leave the delta.
    // Reset mid-debounce: never sync/post into the migration. The arm
    // IS consumed (stale would abort the post-reset retry).
    if crate::handlers::reset::is_resetting() {
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        return;
    }
    // Pre-sync re-check (window = screen read above): a /cancel clearing
    // debounce or a newer arm superseding must abort before sync_topic
    // mints/posts. Exact-arm match — missing means cancelled.
    {
        let db = s.debounce.lock().await;
        if db
            .get(&pane)
            .map(|(st, at)| st != &settled || at != &armed_at)
            .unwrap_or(true)
        {
            return;
        }
    }
    // Liveness before sync (fail-closed verdict in spontaneous):
    // a dead pane never re-mints its topic (resurrection) nor buzzes
    // post-cancel; an ambiguous read mints/posts nothing. Dead consumes
    // the arm (no retry into a gone pane); ambiguous retries bounded
    // (split: `cards_retry`) then leaves the arm for the next transition.
    let live = super::spontaneous::liveness(list_panes(&s.cfg.socket).await.ok().as_ref(), &pane);
    let live = if matches!(live, super::spontaneous::Liveness::Ambiguous) {
        let Some(retry) = super::cards_retry::liveness_retry(&s, &pane, armed_at).await else {
            return;
        };
        retry
    } else {
        live
    };
    match live {
        super::spontaneous::Liveness::Dead => {
            consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
            return;
        }
        super::spontaneous::Liveness::Ambiguous => return,
        super::spontaneous::Liveness::Allow => {}
    }
    let info = get_agent(&s.cfg.socket, &pane).await.ok();
    let (kind, ws_id) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone()),
        None => ("?".into(), "?".into()),
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    // Degraded-spaces guard (status parity): an unmapped ws would prune
    // with the raw id and mint a `[w8]` stub on outage — skip it. Kind
    // "?" (unreadable agent) is already refused inside ensure_inner.
    let spaces_ok = kind == "?" || spaces.iter().any(|w| w.id == ws_id);
    // Single stray chars (picker echoes, vim residue) never page; real
    // shorts ("ok", "done") do. Strays advance the baseline so the same
    // stray doesn't re-RPC every settle — but never an outage/blank read
    // (settle_stray guards via anchorable_screen): that would wipe a good
    // baseline and repost scrollback as fresh (split: `settle_stray` —
    // 300-line file limit).
    if body.chars().count() < 2 {
        super::cards_retry::settle_stray(&s, &pane, &settled, armed_at, screen).await;
        return;
    }
    // Pre-post re-check (window = read RPCs above): a job/final, new
    // work (moved-on), /cancel or newer arm in that window aborts.
    // Same idle↔done collapse as the arm check; exact-arm match.
    if s.job_live(&pane).await {
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        return;
    }
    if s.debounce
        .lock()
        .await
        .get(&pane)
        .map(|(st, at)| st != &settled || at != &armed_at)
        .unwrap_or(true)
    {
        return;
    }
    if super::retry_guard::moved_on(
        s.status.lock().await.get(&pane).map(String::as_str),
        &settled,
    ) {
        // Moved on: exact-consume only (a newer arm survives).
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        return;
    }
    if s.last_done
        .lock()
        .await
        .get(&pane)
        .map(|t| *t > armed_at)
        .unwrap_or(false)
    {
        // Superseded by a delivered final: exact-consume only.
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        return;
    }
    // Pruned topics retire the dialog before posting into the recreated topic.
    // After the guards (aborted arms mint/retire nothing); skipped on degraded spaces.
    if spaces_ok && s.topics.sync_topic_prune(&pane, &kind, raw_space).await.1 {
        crate::handlers::dialog::retire_dialog(&s, &pane).await;
    }
    // Bounded retry on SEND outage only (a blip must not eat a one-shot
    // reply). Two extra tries, then the next transition owns it.
    // Refusals (takeover, newer last_done/arm, moved-on) break instead
    // of spinning; reset aborts the loop.
    let mut delivered = false;
    for i in 0..3 {
        if crate::handlers::reset::is_resetting() {
            break;
        }
        // Post-sleep re-check (window = 15s inter-retry sleep): work
        // starting mid-sleep owns the pane now — moved-on stays silent.
        // First iteration already passed the pre-post guards above; the
        // post itself re-checks jobs/arm/done before every send.
        if i > 0 && super::retry_guard::moved_on_now(&s, &pane, &settled).await {
            break;
        }
        if post_spontaneous_card(
            &s,
            &pane,
            &kind,
            raw_space,
            &settled,
            &body,
            Some(armed_at),
            spaces_ok,
        )
        .await
        {
            delivered = true;
            break;
        }
        if s.job_live(&pane).await {
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
        // A newer arm owns the reply now, and a /cancel clearing the arm
        // aborts too (stale body must not beat either). Exact-arm match.
        if s.debounce
            .lock()
            .await
            .get(&pane)
            .map(|(st, at)| st != &settled || at != &armed_at)
            .unwrap_or(true)
        {
            break;
        }
        // Spontaneous new work started mid-retry: down-window silence.
        if super::retry_guard::moved_on_now(&s, &pane, &settled).await {
            break;
        }
        // No trailing sleep after the final attempt: the loop exits with
        // delivered=false and the next transition owns it.
        if i < 2 {
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }
    if delivered {
        consume_reset_arm(&mut *s.debounce.lock().await, &pane, armed_at);
        s.seen.lock().await.insert(pane.clone(), screen);
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
