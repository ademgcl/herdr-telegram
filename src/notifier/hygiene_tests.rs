//! Tests for dead-pane reaping (split: 300-line file limit).
use super::*;
use crate::{
    jobs::job::Job,
    state::{
        cancel::isolated_state,
        guard::{BLOCKOP_STALE_SECS, KEYWAIT_STALE_SECS, MODELOP_STALE_SECS, TYPEWAIT_STALE_SECS},
    },
};
use std::time::{Duration, Instant};

#[tokio::test]
async fn test_reap_keeps_live_clears_dead() {
    // Injected pane list: no herdr RPC — dead panes retire, live
    // panes (jobs, intent, maps) survive untouched.
    let (s, _dir) = isolated_state();
    let dead_job = Job::new(vec![], 1, None);
    let live_job = Job::new(vec![], 1, None);
    s.jobs
        .lock()
        .await
        .insert("dead:p1".into(), dead_job.clone());
    s.jobs
        .lock()
        .await
        .insert("live:p1".into(), live_job.clone());
    s.status
        .lock()
        .await
        .insert("dead:p1".into(), "working".into());
    s.status
        .lock()
        .await
        .insert("live:p1".into(), "working".into());
    let mut cache = Some(HashSet::from(["live:p1".to_string()]));
    reap_orphans(&s, &mut cache).await;
    assert!(!s.jobs.lock().await.contains_key("dead:p1"));
    assert!(dead_job.is_stopped());
    assert!(s.jobs.lock().await.contains_key("live:p1"));
    assert!(!live_job.is_stopped());
    assert!(!s.status.lock().await.contains_key("dead:p1"));
    assert!(s.status.lock().await.contains_key("live:p1"));
}

#[tokio::test]
async fn test_reap_empty_list_is_fail_open() {
    // Transient Ok([]) must read as "unknown", never "all dead".
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("w1:p1".into(), job.clone());
    let mut cache = Some(HashSet::new());
    reap_orphans(&s, &mut cache).await;
    assert!(s.jobs.lock().await.contains_key("w1:p1"));
    assert!(!job.is_stopped());
}

#[tokio::test]
async fn test_reap_clears_stale_guards_keeps_fresh() {
    // Prod wiring: live-stale corpses reap under their own
    // threshold, live-fresh survives, dead entries vanish.
    let (s, _dir) = isolated_state();
    let now = Instant::now();
    let old_tap = now - Duration::from_secs(BLOCKOP_STALE_SECS + 60);
    let old_model = now - Duration::from_secs(MODELOP_STALE_SECS + 60);
    s.blockop.lock().await.insert("live:p1".into(), old_tap);
    s.blockop.lock().await.insert("live:p2".into(), now);
    s.modelop.lock().await.insert("live:p1".into(), old_model);
    s.modelop.lock().await.insert("live:p2".into(), now);
    s.blockop.lock().await.insert("dead:p9".into(), now);
    s.modelop.lock().await.insert("dead:p9".into(), now);
    let mut cache = Some(HashSet::from([
        "live:p1".to_string(),
        "live:p2".to_string(),
    ]));
    reap_orphans(&s, &mut cache).await;
    assert!(!s.blockop.lock().await.contains_key("live:p1"));
    assert!(s.blockop.lock().await.contains_key("live:p2"));
    assert!(!s.modelop.lock().await.contains_key("live:p1"));
    assert!(s.modelop.lock().await.contains_key("live:p2"));
    assert!(!s.blockop.lock().await.contains_key("dead:p9"));
    assert!(!s.modelop.lock().await.contains_key("dead:p9"));
}

#[tokio::test]
async fn test_reap_retains_dead_reply_targets() {
    // Fail-visible corpses: a DM reply to a dead pane's card must
    // keep its address so routing fails loudly ("pane gone") instead
    // of silently prompting the focused live agent.
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("live:p1".into(), job);
    s.targets.lock().await.insert((1, 10), "live:p1".into());
    s.targets.lock().await.insert((1, 11), "dead:p9".into());
    s.torder.lock().await.push_back((1, 10));
    s.torder.lock().await.push_back((1, 11));
    let mut cache = Some(HashSet::from(["live:p1".to_string()]));
    reap_orphans(&s, &mut cache).await;
    assert_eq!(
        s.targets.lock().await.get(&(1, 10)).map(String::as_str),
        Some("live:p1")
    );
    assert_eq!(
        s.targets.lock().await.get(&(1, 11)).map(String::as_str),
        Some("dead:p9")
    );
}

#[tokio::test]
async fn test_reap_keeps_armed_runwait() {
    // runwait holds workspace ids, not panes: the tick must never
    // misread one as a dead pane and wipe the armed shell-run waiter
    // (the next message is an acknowledged command, not a prompt).
    // Age-expiry still applies: a waiter armed arbitrarily long ago
    // must not fire stale input as a shell command.
    use std::time::{Duration, Instant};
    let (s, _dir) = isolated_state();
    let job = Job::new(vec![], 1, None);
    s.jobs.lock().await.insert("live:p1".into(), job);
    s.runwait
        .lock()
        .await
        .insert((1, None), ("w8".into(), Instant::now()));
    s.runwait.lock().await.insert(
        (2, None),
        ("w8".into(), Instant::now() - Duration::from_secs(3600)),
    );
    let mut cache = Some(HashSet::from(["live:p1".to_string()]));
    reap_orphans(&s, &mut cache).await;
    assert_eq!(
        s.runwait
            .lock()
            .await
            .get(&(1, None))
            .map(|(ws, _)| ws.as_str()),
        Some("w8")
    );
    assert!(!s.runwait.lock().await.contains_key(&(2, None)));
}

#[tokio::test]
async fn test_reap_expires_stale_key_type_waiters() {
    // Armed K/Type waiters must not fire arbitrarily later: age expiry
    // drops them even on live panes (pane-death reap is separate).
    // Fresh arms survive; expired ones go even when the pane is live.
    let (s, _dir) = isolated_state();
    let now = Instant::now();
    s.keywait.lock().await.insert(
        (1, None),
        (
            "live:p1".into(),
            now - Duration::from_secs(KEYWAIT_STALE_SECS + 60),
        ),
    );
    s.keywait
        .lock()
        .await
        .insert((2, None), ("live:p1".into(), now));
    s.typewait.lock().await.insert(
        (3, None),
        (
            "live:p1".into(),
            now - Duration::from_secs(TYPEWAIT_STALE_SECS + 60),
        ),
    );
    s.typewait
        .lock()
        .await
        .insert((4, None), ("live:p1".into(), now));
    let mut cache = Some(HashSet::from(["live:p1".to_string()]));
    reap_orphans(&s, &mut cache).await;
    assert!(!s.keywait.lock().await.contains_key(&(1, None)));
    assert!(s.keywait.lock().await.contains_key(&(2, None)));
    assert!(!s.typewait.lock().await.contains_key(&(3, None)));
    assert!(s.typewait.lock().await.contains_key(&(4, None)));
}

#[tokio::test]
async fn test_reap_prunes_live_maps_when_idle() {
    // Idle bot (no jobs, intents, waiters, or guards) must still prune
    // live-only maps — otherwise dead panes leak their entries forever.
    use std::collections::VecDeque;
    let (s, _dir) = isolated_state();
    s.seen
        .lock()
        .await
        .insert("dead:p9".into(), vec!["x".into()]);
    s.seen
        .lock()
        .await
        .insert("live:p1".into(), vec!["x".into()]);
    s.history
        .lock()
        .await
        .insert("dead:p9".into(), VecDeque::from(["h".to_string()]));
    let mut cache = Some(HashSet::from(["live:p1".to_string()]));
    reap_orphans(&s, &mut cache).await;
    assert!(!s.seen.lock().await.contains_key("dead:p9"));
    assert!(s.seen.lock().await.contains_key("live:p1"));
    assert!(!s.history.lock().await.contains_key("dead:p9"));
}

#[tokio::test]
async fn test_reap_prunes_stale_intent_and_persists() {
    // In-uptime prune uses boot-recover's bound (stale >24h, future
    // >1h): dead or alive, unrecoverable intents drop — and the prune
    // persists like any clear, or the next boot re-arms the corpse.
    use crate::jobs::persist::PendingPrompt;
    use std::time::{SystemTime, UNIX_EPOCH};
    let (s, _dir) = isolated_state();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pp = |started_unix| PendingPrompt {
        chat: 1,
        thread: None,
        prompt: "hi".into(),
        started_unix,
    };
    s.pending
        .lock()
        .await
        .insert("dead:old".into(), pp(now - 90000));
    s.pending
        .lock()
        .await
        .insert("dead:future".into(), pp(now + 7200));
    s.pending.lock().await.insert("dead:fresh".into(), pp(now));
    s.pending
        .lock()
        .await
        .insert("live:old".into(), pp(now - 90000));
    s.pending.lock().await.insert("live:fresh".into(), pp(now));
    let mut cache = Some(HashSet::from([
        "live:old".to_string(),
        "live:fresh".to_string(),
    ]));
    reap_orphans(&s, &mut cache).await;
    let m = s.pending.lock().await;
    assert!(!m.contains_key("dead:old"));
    assert!(!m.contains_key("dead:future"));
    assert!(m.contains_key("dead:fresh"));
    assert!(!m.contains_key("live:old"));
    assert!(m.contains_key("live:fresh"));
    let mem = m.clone();
    drop(m);
    // Persisted like any clear: roundtrip the pruned map through an
    // explicit path (the reap itself saves via the shared state dir,
    // which parallel tests re-point per isolated_state — see cancel.rs
    // — so no test reads that file back).
    let dir = std::env::temp_dir().join(format!(
        "ht-prune-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("prune tempdir");
    let file = dir.join("jobs.state");
    crate::jobs::persist::save_file(&file, &mem);
    let disk: std::collections::HashMap<String, PendingPrompt> =
        crate::jobs::persist::load_file(&file);
    assert!(!disk.contains_key("dead:old"));
    assert!(!disk.contains_key("live:old"));
    assert!(disk.contains_key("live:fresh"));
    assert!(disk.contains_key("dead:fresh"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_confirm_deaths_union_verified() {
    // Deferred transient handling, now verified: a pane the first read
    // missed but the confirm sees survives AND joins the retain set
    // (pruning on one read's miss would wipe live baselines/dedup);
    // a pane missing from both reads is truly dead.
    let first: HashSet<String> = HashSet::from(["live:p1".to_string()]);
    let fresh: HashSet<String> = HashSet::from(["live:p1".to_string(), "flap:p2".to_string()]);
    let dying = vec!["flap:p2".to_string(), "dead:p9".to_string()];
    let (out, retain) = super::confirm_deaths(&first, &fresh, dying);
    assert_eq!(out, vec!["dead:p9".to_string()]);
    assert!(retain.contains("live:p1"));
    assert!(retain.contains("flap:p2"));
    assert!(!retain.contains("dead:p9"));
}
