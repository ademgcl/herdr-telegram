//! Guard unit tests: split from `guard` (300-line file limit).
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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
    assert!(!claim_stale(
        now - Duration::from_secs(3),
        now,
        BLOCKOP_STALE_SECS
    ));
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
    assert_eq!(
        reaped,
        vec!["dead-stale".to_string(), "live-stale".to_string()]
    );
    assert!(map.contains_key("live-fresh"));
    assert!(!map.contains_key("live-stale"));
    assert!(!map.contains_key("dead-fresh"));
    assert!(!map.contains_key("dead-stale"));
}
