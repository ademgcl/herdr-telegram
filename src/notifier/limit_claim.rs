//! Drop-backstop for the watchdog limit-stall claim (split from
//! `limits`: 300-line file limit). The claim is inserted pre-send so a
//! concurrent watcher tick stays silent — but the 50s watchdog timeout
//! in `main` drops `reconcile` mid-send, bypassing the failure-release.
//! The leaked claim then suppresses the stall for a full 30-min remind
//! window with nothing delivered. The guard releases iff still ours on
//! drop (best-effort `try_lock`: no await in `Drop`); explicit
//! keep/release cover the live paths. Watcher-side claims (`stall.rs`)
//! need no guard: watchers retire cooperatively, never dropped mid-send.
use crate::{
    notifier::LIMIT_REMIND_SECS, notifier::limit_decide::alert_suppressed, state::AppState,
};
use std::time::Instant;

/// Pre-send claim for one limit-stall buzz. Drop releases iff still
/// ours — a concurrent success landing after our claim must survive.
pub(crate) struct LimitClaim<'a> {
    s: &'a AppState,
    pane: String,
    kind: String,
    at: Instant,
    armed: bool,
}

/// Atomic check-and-claim (shared verdict with the watcher path): same
/// kind within the remind window stays silent, otherwise claim and send.
/// `None` = suppressed, no guard. `Some` = ours, must keep or release.
pub(crate) async fn try_claim<'a>(
    s: &'a AppState,
    pane: &str,
    kind: &str,
    now: Instant,
) -> Option<LimitClaim<'a>> {
    // Pre-send, but REMOVED on failure/drop below, so drops never arm
    // the 30-min suppress.
    let dup = {
        let mut map = s.limit_alert.lock().await;
        let prev = map.get(pane).map(|(k, t)| (k.clone(), *t));
        if alert_suppressed(
            kind,
            prev.as_ref().map(|(k, t)| (k.as_str(), *t)),
            now,
            LIMIT_REMIND_SECS,
        ) {
            true
        } else {
            map.insert(pane.to_string(), (kind.to_string(), now));
            false
        }
    };
    if dup {
        return None;
    }
    Some(LimitClaim {
        s,
        pane: pane.to_string(),
        kind: kind.to_string(),
        at: now,
        armed: true,
    })
}

impl LimitClaim<'_> {
    /// Delivered: keep the claim (arms the remind suppress), disarm.
    pub(crate) fn keep(mut self) {
        self.armed = false;
    }

    /// Send failed: release iff still ours (a concurrent success must
    /// survive) and cool down — the next tick retries, nothing
    /// suppresses, nothing storms.
    pub(crate) async fn release(mut self) {
        // Disarm AFTER the removal: a task dropped during the lock
        // await above must land in Drop still armed (best-effort
        // removal) — disarming first leaks the claim for the full
        // remind window with nothing delivered.
        let mut map = self.s.limit_alert.lock().await;
        if matches!(map.get(&self.pane), Some((k, t)) if k == &self.kind && *t == self.at) {
            map.remove(&self.pane);
        }
        self.armed = false;
    }
}

impl Drop for LimitClaim<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Best-effort only: no await in `Drop`. A contested lock keeps
        // the claim, which SUPPRESSES the same kind for the remind
        // window (silence, not a loud dup — see module docs). The live
        // paths always release explicitly above; every critical section
        // here is synchronous map ops, so contention is near-impossible.
        if let Ok(mut map) = self.s.limit_alert.try_lock()
            && matches!(map.get(&self.pane), Some((k, t)) if k == &self.kind && *t == self.at)
        {
            map.remove(&self.pane);
        }
    }
}

#[cfg(test)]
#[path = "limit_claim_tests.rs"]
mod tests;
