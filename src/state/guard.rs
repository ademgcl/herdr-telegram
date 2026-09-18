//! Single-flight op guards: one tap/switch owns a pane until it lands.
//! Split from `state` (300-line file limit).
//!
//! Why timestamps: every answer path (taps, typed answers, /card, /esc)
//! refuses while `blockop` holds the pane, so a guard that never drops
//! bricks the pane until restart — exactly the stuck-second-question
//! bug (first tap posts the new card, its drop then loses the lock
//! race and the pane stays "in flight" forever). Three layers:
//! * `Drop` spins boundedly (holders never hold across awaits, so
//!   contention is microsecond-scale — a single `try_lock` was the hole).
//! * claims carry instants and self-evict stale entries, so the next
//!   user action rescues immediately instead of waiting for a tick.
//! * peeks (`is_held` / `block_held` / `model_held`) evict stale on
//!   read too — every refuse path (/card, typewait, /keys, observations)
//!   heals the same way claims do; the hygiene tick is only a backstop.
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Corpse threshold for tap claims (see module docs): far above the
/// worst legit hold (stacked RPC timeouts ≈ 3min), far below
/// "stuck until restart".
pub(crate) const BLOCKOP_STALE_SECS: u64 = 300;

/// Same for model switches, whose worst pileup runs longer (picker
/// RPCs + sleeps ≈ 6min all-timing-out) — a shorter reap could eat a
/// live switch.
pub(crate) const MODELOP_STALE_SECS: u64 = 900;

/// Armed shell-run waiter lifetime: the R button's "next message is a
/// command" must not fire arbitrarily later (stale input executing
/// writes). Far above legit arm→type gaps (seconds), far below
/// "stuck until restart".
pub(crate) const RUNWAIT_STALE_SECS: u64 = 900;

/// Armed raw-keys waiter lifetime: stale keys executing into a live
/// session is the dangerous half of waiter staleness — same bound as
/// runwait.
pub(crate) const KEYWAIT_STALE_SECS: u64 = 900;

/// Armed typed-answer waiter lifetime: generous — answers take a while
/// to compose, and expiry degrades gracefully to normal routing (in a
/// topic the bare-text path types into blocked panes anyway).
pub(crate) const TYPEWAIT_STALE_SECS: u64 = 3600;

/// Bounded drop-spin budget (holders release immediately, never across
/// awaits — contention is microsecond-scale; this is fail-safe, not
/// load-bearing). Small + `spin_loop` (never `yield_now`/sleep): must
/// not block the tokio worker it runs on.
const DROP_SPIN_ITERS: u32 = 1_000;

/// Pure corpse predicate so tests cover the reap line without sleeps.
pub(crate) fn claim_stale(since: Instant, now: Instant, limit_secs: u64) -> bool {
    now.duration_since(since) >= Duration::from_secs(limit_secs)
}

/// RAII single-flight guard: the pane is removed from the map on drop —
/// including task cancellation between insert and the manual remove —
/// so a wedged insert can never brick the pane until restart. Removal
/// is generation-checked (only our own instant): an evict-while-running
/// successor is never deleted by our late drop.
pub struct OpGuard<'a> {
    set: &'a Mutex<HashMap<String, Instant>>,
    pane: String,
    at: Instant,
}

impl<'a> OpGuard<'a> {
    /// Atomically claim the pane with the tap threshold; `None` means
    /// already in flight.
    pub async fn claim(set: &'a Mutex<HashMap<String, Instant>>, pane: &str) -> Option<Self> {
        Self::claim_limited(set, pane, BLOCKOP_STALE_SECS).await
    }

    /// Claim with an explicit corpse threshold (model switches run
    /// longer than taps). Check-then-insert under one lock hold: a
    /// refused claim is read-only and must never re-stamp the live
    /// claim's instant (retry load would keep a corpse forever young).
    /// A stale entry self-evicts here, so the next user action rescues
    /// immediately instead of waiting for the hygiene tick.
    pub async fn claim_limited(
        set: &'a Mutex<HashMap<String, Instant>>,
        pane: &str,
        limit_secs: u64,
    ) -> Option<Self> {
        // Evict under the lock, log after drop (never hold a guard
        // across I/O — slow stderr would widen the Drop race window).
        let (evicted, at) = {
            let mut map = set.lock().await;
            let mut evicted = false;
            if let Some(since) = map.get(pane) {
                if !claim_stale(*since, Instant::now(), limit_secs) {
                    return None;
                }
                map.remove(pane);
                evicted = true;
            }
            let at = Instant::now();
            map.insert(pane.to_string(), at);
            (evicted, at)
        };
        if evicted {
            eprintln!("[state] evicting stale guard for {pane}");
        }
        Some(Self {
            set,
            pane: pane.to_string(),
            at,
        })
    }

    /// Self-healing peek: true only while a FRESH claim holds the pane.
    /// A stale entry evicts here (same threshold as claims), so refuse
    /// paths (/card, typewait, /keys, observations) rescue on the next
    /// user action instead of waiting for the hygiene tick. Refused
    /// (fresh) peeks are read-only and never re-stamp the live instant.
    pub async fn is_held(
        set: &Mutex<HashMap<String, Instant>>,
        pane: &str,
        limit_secs: u64,
    ) -> bool {
        // Same log-after-drop rule as claims.
        let evicted = {
            let mut map = set.lock().await;
            match map.get(pane) {
                None => return false,
                Some(since) if claim_stale(*since, Instant::now(), limit_secs) => {
                    map.remove(pane);
                    true
                }
                Some(_) => return true,
            }
        };
        if evicted {
            eprintln!("[state] evicting stale guard for {pane}");
        }
        false
    }
}

impl super::State {
    /// Tap guard held (fresh only — stale self-evicts, see `is_held`).
    pub(crate) async fn block_held(&self, pane: &str) -> bool {
        OpGuard::is_held(&self.blockop, pane, BLOCKOP_STALE_SECS).await
    }

    /// Model-switch guard held (own longer threshold).
    pub(crate) async fn model_held(&self, pane: &str) -> bool {
        OpGuard::is_held(&self.modelop, pane, MODELOP_STALE_SECS).await
    }
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        // No await in Drop and never blocks the executor: bounded CPU
        // spin only (holders release immediately, never across
        // awaits — and evict paths now log after drop, so no I/O ever
        // widens this window). A single try_lock here was the
        // stuck-answers bug — one lost race wedged the pane, and every
        // answer path (`tap already in flight`, `/card`, `/esc`, typed
        // answers) refuses while the map holds it. Generation-checked:
        // an evict-while-running successor keeps its guard when our
        // late drop lands.
        for _ in 0..DROP_SPIN_ITERS {
            if let Ok(mut set) = self.set.try_lock() {
                if set.get(&self.pane).map(|at| *at == self.at).unwrap_or(false) {
                    set.remove(&self.pane);
                }
                return;
            }
            std::hint::spin_loop();
        }
        eprintln!("[state] OpGuard drop wedged for {}", self.pane);
    }
}

/// Reap dead panes and corpse claims: drops entries that are gone,
/// plus live entries older than `limit_secs` (a wedged tap guard must
/// never brick answers until restart). Returns reaped panes for loud
/// logging. Shared by the hygiene tick (prod) and unit tests.
pub(crate) fn reap_stale(
    map: &mut HashMap<String, Instant>,
    live: &HashSet<String>,
    now: Instant,
    limit_secs: u64,
) -> Vec<String> {
    let mut reaped = Vec::new();
    map.retain(|p, at| {
        let stale = claim_stale(*at, now, limit_secs);
        if stale {
            reaped.push(p.clone());
        }
        live.contains(p) && !stale
    });
    reaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_claim_drop_reclaims() {
        let m = Mutex::new(HashMap::new());
        {
            let _g = OpGuard::claim(&m, "w1:p1").await.expect("first claims");
            assert!(m.lock().await.contains_key("w1:p1"));
            assert!(OpGuard::claim(&m, "w1:p1").await.is_none());
        }
        assert!(!m.lock().await.contains_key("w1:p1"));
        assert!(OpGuard::claim(&m, "w1:p1").await.is_some());
    }

    #[test]
    fn test_claim_stale_threshold() {
        let now = Instant::now();
        let old = now - Duration::from_secs(BLOCKOP_STALE_SECS + 1);
        assert!(claim_stale(old, now, BLOCKOP_STALE_SECS));
        assert!(!claim_stale(now, now, BLOCKOP_STALE_SECS));
        assert!(!claim_stale(now - Duration::from_secs(3), now, BLOCKOP_STALE_SECS));
    }

    #[tokio::test]
    async fn test_claim_evicts_stale_keeps_fresh() {
        // A corpse never blocks the next tap; a live claim still refuses.
        let m = Mutex::new(HashMap::new());
        let ancient = Instant::now() - Duration::from_secs(BLOCKOP_STALE_SECS + 60);
        m.lock().await.insert("old".to_string(), ancient);
        m.lock().await.insert("new".to_string(), Instant::now());
        assert!(OpGuard::claim(&m, "old").await.is_some());
        assert!(OpGuard::claim(&m, "new").await.is_none());
        // Refused claims are read-only: retry load must not re-stamp.
        let at = *m.lock().await.get("new").unwrap();
        assert!(OpGuard::claim(&m, "new").await.is_none());
        assert_eq!(*m.lock().await.get("new").unwrap(), at);
    }

    #[tokio::test]
    async fn test_is_held_evicts_stale_keeps_fresh() {
        // Peek paths (/card, typewait, /keys) heal the same way claims
        // do; fresh peeks stay read-only (no re-stamp).
        let m = Mutex::new(HashMap::new());
        let ancient = Instant::now() - Duration::from_secs(BLOCKOP_STALE_SECS + 60);
        m.lock().await.insert("old".to_string(), ancient);
        m.lock().await.insert("new".to_string(), Instant::now());
        assert!(!OpGuard::is_held(&m, "old", BLOCKOP_STALE_SECS).await);
        assert!(!m.lock().await.contains_key("old"));
        assert!(OpGuard::is_held(&m, "new", BLOCKOP_STALE_SECS).await);
        let at = *m.lock().await.get("new").unwrap();
        assert!(OpGuard::is_held(&m, "new", BLOCKOP_STALE_SECS).await);
        assert_eq!(*m.lock().await.get("new").unwrap(), at);
        assert!(!OpGuard::is_held(&m, "missing", BLOCKOP_STALE_SECS).await);
    }

    #[tokio::test]
    async fn test_late_drop_keeps_successor() {
        // Evict-while-running: B rescues a wedged A, then A's leaked
        // task finally drops — B's live guard must survive (a third
        // claimer must still refuse while B runs).
        let m = Mutex::new(HashMap::new());
        let g1 = OpGuard::claim(&m, "w1:p1").await.expect("first claims");
        *m.lock().await.get_mut("w1:p1").unwrap() =
            Instant::now() - Duration::from_secs(BLOCKOP_STALE_SECS + 60);
        let g2 = OpGuard::claim(&m, "w1:p1").await.expect("evicts wedged A");
        drop(g1);
        assert!(m.lock().await.contains_key("w1:p1"));
        assert!(OpGuard::claim(&m, "w1:p1").await.is_none());
        drop(g2);
        assert!(!m.lock().await.contains_key("w1:p1"));
    }

    #[test]
    fn test_reap_keeps_live_fresh_drops_stale_dead() {
        let now = Instant::now();
        let old = now - Duration::from_secs(BLOCKOP_STALE_SECS + 60);
        let mut map = HashMap::from([
            ("live-fresh".to_string(), now),
            ("live-stale".to_string(), old),
            ("dead-fresh".to_string(), now),
            ("dead-stale".to_string(), old),
        ]);
        let live = HashSet::from(["live-fresh".to_string(), "live-stale".to_string()]);
        let mut reaped = reap_stale(&mut map, &live, now, BLOCKOP_STALE_SECS);
        reaped.sort();
        assert_eq!(reaped, vec!["dead-stale".to_string(), "live-stale".to_string()]);
        assert!(map.contains_key("live-fresh"));
        assert!(!map.contains_key("live-stale"));
        assert!(!map.contains_key("dead-fresh"));
        assert!(!map.contains_key("dead-stale"));
    }
}
