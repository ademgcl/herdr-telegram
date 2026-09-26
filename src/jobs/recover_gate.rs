//! Boot-recover pure gates (split from `recover`, 300-line file limit).
use crate::jobs::job::Job;

/// Boot-rearm eligibility, pure so tests pin it: stale (>24h) or
/// future-dated (>1h ahead — a clock jump forward then corrected would
/// otherwise re-arm a dead prompt on every boot forever) intents never
/// re-arm.
pub fn recoverable(started_unix: u64, now: u64) -> bool {
    now.saturating_sub(started_unix) <= 86400 && started_unix <= now.saturating_add(3600)
}

/// Stale-drop clear verdict (pure, tested): delivery-gated, but bounded
/// so a dead Telegram never retries forever. Clear when the notice
/// landed, or when the intent is older than 7d / further than 7d in the
/// future (clock-jump corpse) — disk + boot scans stay bounded either
/// way. Single source for the stale arm below.
pub const STALE_KEEP_MAX_SECS: u64 = 7 * 86400;

pub fn stale_drop_should_clear(delivered: bool, started_unix: u64, now: u64) -> bool {
    if delivered {
        return true;
    }
    if now.saturating_sub(started_unix) > STALE_KEEP_MAX_SECS {
        return true;
    }
    if started_unix.saturating_sub(now) > STALE_KEEP_MAX_SECS {
        return true;
    }
    false
}

/// Atomic watcher claim: check + insert under the caller's single
/// jobs-lock hold. Returns false when a LIVE watcher already owns the pane
/// (a concurrent re-arm won the race) — caller must stand down, never
/// run two watchers. A stopped corpse never blocks a re-arm (enqueue
/// leaves stopped jobs in the map; they are replaced, never reused).
/// Pure over the map so tests pin the verdict.
pub(crate) fn claim_watcher(
    jobs: &mut std::collections::HashMap<String, std::sync::Arc<Job>>,
    pane: &str,
    job: std::sync::Arc<Job>,
) -> bool {
    if jobs.get(pane).is_some_and(|j| !j.is_stopped()) {
        return false;
    }
    jobs.insert(pane.to_string(), job);
    true
}
