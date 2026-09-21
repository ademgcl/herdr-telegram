//! Bounded retry reads for the spontaneous settle check (split from
//! `cards`: 300-line file limit). One-shot returns on ambiguous reads
//! lose the reply forever — no new transition re-fires the arm — so both
//! helpers take two extra passes over a few seconds before giving up (a
//! lasting outage leaves the arm for the next transition). Fail-closed
//! throughout: never anchor, never mint, never post.
use crate::{
    herdr::client::{list_panes, read_agent_output, read_agent_visible},
    notifier::spontaneous::{Liveness, liveness},
    state::AppState,
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

/// Seconds between retry passes (transient blips, never a tight loop).
const RETRY_SECS: u64 = 5;
/// Extra passes after the first ambiguous read.
const RETRIES: u32 = 2;

/// Spontaneous screen read (single source for `cards` + the retry below):
/// `recent_unwrapped` first (80-line settle window), visible-viewport
/// fallback for busy alt-screen TUIs (finalize parity — recent-only
/// reads error there and the reply would be lost with no new transition
/// to re-fire the arm). Empty means outage/unknown on both sources.
pub(crate) async fn read_screen_spontaneous(s: &AppState, pane: &str) -> Vec<String> {
    let recent: Vec<String> = read_agent_output(&s.cfg.socket, pane, 80)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    if !recent.is_empty() {
        return recent;
    }
    read_agent_visible(&s.cfg.socket, pane, 60)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Re-read an empty screen. `None` = caller returns at once (raced with
/// a job/final, reset started, or the outage lasts — the arm stays for
/// the next transition); `Some` is always non-empty.
pub(crate) async fn read_screen_retry(
    s: &AppState,
    pane: &str,
    armed_at: Instant,
) -> Option<Vec<String>> {
    for _ in 0..RETRIES {
        tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
        if crate::handlers::reset::is_resetting() {
            consume_reset_arm(&mut *s.debounce.lock().await, pane, armed_at);
            return None;
        }
        if s.job_live(pane).await {
            return None;
        }
        if s.last_done
            .lock()
            .await
            .get(pane)
            .map(|t| *t > armed_at)
            .unwrap_or(false)
        {
            return None;
        }
        let screen = read_screen_spontaneous(s, pane).await;
        if !screen.is_empty() {
            return Some(screen);
        }
    }
    None
}

/// Stray/empty settle arm (split from `cards`, 300-line file limit):
/// single stray chars never page, but the baseline must still advance or
/// the same stray re-RPCs every settle forever. Post-RPC re-checks (a
/// prompt/final, moved-on work, or a newer arm landing during the reads
/// above): anchoring would wipe the fresh delta into the baseline (lost
/// reply) — consume the arm only, never the baseline. Checked BEFORE
/// sync_topic_prune: strays post nothing, so they must neither mint a
/// card-less topic nor retire the blocked dialog.
pub(crate) async fn settle_stray(
    s: &AppState,
    pane: &str,
    settled: &str,
    armed_at: Instant,
    screen: Vec<String>,
) {
    // Newer arm superseding in the read window owns the reply (pre-post
    // parity: exact-arm match, arm left for the new owner).
    if s.debounce
        .lock()
        .await
        .get(pane)
        .map(|(st, at)| st != settled || at != &armed_at)
        .unwrap_or(true)
    {
        return;
    }
    let raced = s.job_live(pane).await
        || s.last_done
            .lock()
            .await
            .get(pane)
            .map(|t| *t > armed_at)
            .unwrap_or(false)
        || super::retry_guard::moved_on(
            s.status.lock().await.get(pane).map(String::as_str),
            settled,
        );
    if raced {
        consume_reset_arm(&mut *s.debounce.lock().await, pane, armed_at);
        return;
    }
    consume_reset_arm(&mut *s.debounce.lock().await, pane, armed_at);
    s.seen.lock().await.insert(pane.to_string(), screen);
}

/// Re-poll an ambiguous liveness verdict. `None` = caller returns at
/// once (raced or reset); `Some` carries the verdict — `Ambiguous`
/// included when the outage lasts (caller leaves the arm, never mints).
pub(crate) async fn liveness_retry(
    s: &AppState,
    pane: &str,
    armed_at: Instant,
) -> Option<Liveness> {
    for _ in 0..RETRIES {
        tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
        if crate::handlers::reset::is_resetting() {
            consume_reset_arm(&mut *s.debounce.lock().await, pane, armed_at);
            return None;
        }
        if s.job_live(pane).await {
            return None;
        }
        let live = liveness(list_panes(&s.cfg.socket).await.ok().as_ref(), pane);
        if !matches!(live, Liveness::Ambiguous) {
            return Some(live);
        }
    }
    Some(Liveness::Ambiguous)
}
