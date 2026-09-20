//! Spontaneous send-gate verdicts: split from `spontaneous` (300-line
//! file limit). Pure verdicts so tests pin them; the async gate only
//! snapshots the four inputs.
use crate::state::AppState;
use std::time::Instant;

/// Settle-arm currency (single source for forum + DM inside-post
/// re-checks): missing means cancelled, any status/instant mismatch means
/// superseded. Pure for tests.
pub(crate) fn arm_superseded(
    cur: Option<(&str, &Instant)>,
    settled: &str,
    armed_at: &Instant,
) -> bool {
    cur.map(|(st, at)| st != settled || at != armed_at)
        .unwrap_or(true)
}

/// DM arm currency (single source for the DM inside-post re-check):
/// DM mode never inserts debounce arms (status.rs returns before the
/// forum arm), so a missing arm proceeds — only a present-but-stale
/// arm aborts. The `last_done` check below still suppresses stale
/// duplicates when a final retired during the screen RPC. Pure for
/// tests.
pub(crate) fn dm_arm_blocks(
    cur: Option<(&str, &Instant)>,
    settled: &str,
    armed_at: &Instant,
) -> bool {
    cur.map(|(st, at)| st != settled || at != armed_at)
        .unwrap_or(false)
}

/// Per-part stale-tail verdict (single source for the forum + DM
/// inside-post loops): a submit landing during a slow multi-part
/// flood-wait must stop the stale tail — the pre-loop check alone still
/// posts part 2+ beside the new prompt's turn. Any retarget sign aborts.
/// Pure for tests.
pub(crate) fn part_stale(
    jobs_active: bool,
    arm_blocked: bool,
    done_after: bool,
    moved_on: bool,
) -> bool {
    jobs_active || arm_blocked || done_after || moved_on
}

/// Per-part currency snapshot (single source for the gate below):
/// jobs-active, arm-blocked, done-after. One lock per map, never nested.
/// `dm` selects the arm currency (DM missing-arm proceeds, forum missing
/// aborts). Moved-on is read separately (status lock) at each call site.
async fn part_currency(
    s: &AppState,
    pane: &str,
    settled: &str,
    armed_at: Option<Instant>,
    dm: bool,
) -> (bool, bool, bool) {
    // Live-only (reconcile/typing parity): a stopped corpse between
    // mark_stopped() and map removal must not read as an active watcher
    // (else a stale tail posts beside the new prompt's turn).
    let jobs_active = s
        .jobs
        .lock()
        .await
        .get(pane)
        .is_some_and(|j| !j.is_stopped());
    match armed_at {
        Some(at) => {
            let cur = s.debounce.lock().await.get(pane).cloned();
            let watch = cur.as_ref().map(|(st, a)| (st.as_str(), a));
            let blocked = if dm {
                dm_arm_blocks(watch, settled, &at)
            } else {
                arm_superseded(watch, settled, &at)
            };
            let after = s
                .last_done
                .lock()
                .await
                .get(pane)
                .map(|t| *t > at)
                .unwrap_or(false);
            (jobs_active, blocked, after)
        }
        None => (jobs_active, false, false),
    }
}

async fn part_moved(s: &AppState, pane: &str, settled: &str) -> bool {
    let st = s.status.lock().await;
    super::retry_guard::moved_on(st.get(pane).map(String::as_str), settled)
}

/// Per-part send gate (single source for all three call sites): true
/// when the part must NOT post — a submit landing mid-flood, a
/// superseded arm, a newer final, or a moved-on pane. Pure verdict via
/// `part_stale` (tested); this only snapshots the four inputs.
pub(crate) async fn part_blocked(
    s: &AppState,
    pane: &str,
    settled: &str,
    armed_at: Option<Instant>,
    dm: bool,
) -> bool {
    let (jobs_active, arm_blocked, done_after) =
        part_currency(s, pane, settled, armed_at, dm).await;
    part_stale(
        jobs_active,
        arm_blocked,
        done_after,
        part_moved(s, pane, settled).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arm_superseded_currency() {
        // Exact arm proceeds; missing means cancelled; any mismatch
        // (newer arm, status flip) aborts the stale post.
        let at = Instant::now();
        let later = at + std::time::Duration::from_secs(1);
        assert!(!arm_superseded(Some(("done", &at)), "done", &at));
        assert!(arm_superseded(None, "done", &at));
        assert!(arm_superseded(Some(("done", &later)), "done", &at));
        assert!(arm_superseded(Some(("idle", &at)), "done", &at));
    }

    #[test]
    fn test_dm_arm_blocks_missing_arm_proceeds() {
        // DM mode never inserts debounce arms: missing proceeds so
        // spontaneous answers actually deliver; a present-but-stale
        // arm still aborts the superseded post.
        let at = Instant::now();
        let later = at + std::time::Duration::from_secs(1);
        assert!(!dm_arm_blocks(None, "done", &at));
        assert!(!dm_arm_blocks(Some(("done", &at)), "done", &at));
        assert!(dm_arm_blocks(Some(("done", &later)), "done", &at));
        assert!(dm_arm_blocks(Some(("idle", &at)), "done", &at));
    }

    #[test]
    fn test_part_stale_any_retarget_aborts() {
        // A submit landing mid-flood (jobs_active) stops the stale tail —
        // the pre-loop check alone still posts part 2+ beside the new turn.
        assert!(part_stale(true, false, false, false));
        // Superseded arm, newer final, and moved-on pane all abort too.
        assert!(part_stale(false, true, false, false));
        assert!(part_stale(false, false, true, false));
        assert!(part_stale(false, false, false, true));
        // Quiet pane with nothing racing: the part posts.
        assert!(!part_stale(false, false, false, false));
    }
}
