//! Bounded retry reads for the spontaneous settle check (split from
//! `cards`: 300-line file limit). One-shot returns on ambiguous reads
//! lose the reply forever — no new transition re-fires the arm — so both
//! helpers take two extra passes over a few seconds before giving up (a
//! lasting outage leaves the arm for the next transition). Fail-closed
//! throughout: never anchor, never mint, never post.
use crate::{
    herdr::client::{list_panes, read_agent_output},
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
        let screen: Vec<String> = read_agent_output(&s.cfg.socket, pane, 80)
            .await
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim_end().to_string())
            .collect();
        if !screen.is_empty() {
            return Some(screen);
        }
    }
    None
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
