//! Dead-pane hygiene: mode-independent reaping of jobs, durable intent,
//! and per-pane maps for externally-closed panes. Split from `reconcile`
//! (300-line file limit).
use crate::{herdr::client::list_panes, state::AppState};
use std::collections::HashSet;

/// Cached single `list_panes` per tick (shared with the forum block):
/// 1 RPC, not 2. Fail-open: Err keeps everything.
pub(crate) async fn panes_once(
    s: &AppState,
    cache: &mut Option<HashSet<String>>,
) -> Option<HashSet<String>> {
    if let Some(p) = cache {
        return Some(p.clone());
    }
    match list_panes(&s.cfg.socket).await {
        Ok(l) => {
            let set: HashSet<String> = l.into_iter().collect();
            *cache = Some(set.clone());
            Some(set)
        }
        Err(e) => {
            eprintln!("[reconcile] pane list failed, keeping topics: {e}");
            None
        }
    }
}

/// Reap mode-independent orphans: DM-mode prompts can orphan jobs,
/// durable intent, and per-pane maps for externally-closed panes (no
/// topic mapping exists to trigger the forum close flow). Live panes
/// are skipped; truly gone ones are cancelled + cleared. Fail-open:
/// never wipe intents on a failed list call (Err) or a transient empty
/// Ok([]).
pub(crate) async fn reap_orphans(s: &AppState, pane_list: &mut Option<HashSet<String>>) {
    let mut known: Vec<String> = s.jobs.lock().await.keys().cloned().collect();
    known.extend(s.pending.lock().await.keys().cloned());
    // Armed input waiters also pin a pane: a keywait/typewait for an
    // externally-closed shell (no job, no intent, DM mode) must die
    // with it instead of eating the next message as dead input.
    // runwait is deliberately EXCLUDED: it holds workspace ids, not
    // pane ids — admitting one would misread it as a dead pane and
    // wipe the armed shell-run waiter it was meant to protect.
    known.extend(s.keywait.lock().await.values().cloned());
    known.extend(s.typewait.lock().await.values().cloned());
    // Guard-only wedges pin too: a tap that consumed its waiter (no
    // job, no intent) must still reach the reap below, never idle-skip.
    known.extend(s.blockop.lock().await.keys().cloned());
    known.extend(s.modelop.lock().await.keys().cloned());
    if known.is_empty() {
        return;
    }
    match panes_once(s, pane_list).await {
        Some(live) if live.is_empty() => {
            eprintln!("[reconcile] pane list empty, keeping intents");
        }
        Some(live) => {
            // Retain live-only (not live∪known): known includes
            // the just-cleared dead panes, so ∪ would keep
            // everything clear_pane missed.
            for pane in &known {
                if !live.contains(pane) {
                    // Job-only retire: the durable intent is
                    // boot-recover's reporter for dead panes
                    // ("pane gone before reply arrived") — a loud
                    // retire would wipe it the same tick the
                    // forum branch above restored it, and a
                    // "✋ cancelled" card on a dead pane is noise.
                    // Waiters still die via clear_pane below, maps
                    // via the retains; lingering intent is bounded
                    // by recover's 24h stale drop.
                    s.cancel_job_only_for(pane).await;
                    s.clear_pane(pane).await;
                }
            }
            s.seen.lock().await.retain(|p, _| live.contains(p));
            s.last_done.lock().await.retain(|p, _| live.contains(p));
            // History is RAM-only + bounded per pane, but history-only
            // panes (no job/intent) never enter `known` above — retain
            // live-only or dead panes leak 20 rows each forever.
            s.history.lock().await.retain(|p, _| live.contains(p));
            // status+last_change under one scope (order status→last_change,
            // no awaits inside): an event inserting between torn retains
            // would orphan status without its change instant and skip
            // flap-collapse on the next bounce.
            {
                let mut st = s.status.lock().await;
                let mut lc = s.last_change.lock().await;
                st.retain(|p, _| live.contains(p));
                lc.retain(|p, _| live.contains(p));
            }
            s.limit_alert.lock().await.retain(|p, _| live.contains(p));
            s.limit_seen.lock().await.retain(|p, _| live.contains(p));
            s.limit_miss.lock().await.retain(|p, _| live.contains(p));
            s.limit_send_cool
                .lock()
                .await
                .retain(|p, _| live.contains(p));
            s.debounce.lock().await.retain(|p, _| live.contains(p));
            s.blocked_sig.lock().await.retain(|p, _| live.contains(p));
            s.blocked_card.lock().await.retain(|p, _| live.contains(p));
            // Corpse-tap reap: a wedged OpGuard (drop lost the lock race)
            // must never brick answers until restart — every answer path
            // refuses while blockop holds the pane. Legit taps hold
            // minutes (bounded RPC timeouts, worst ≈3min stacked); anything
            // older is dead. Model switches pile longer (own threshold),
            // same guarantee. Peeks self-evict too — this tick is only
            // the backstop. Log after drop (never hold a guard across I/O).
            {
                use crate::state::guard::{BLOCKOP_STALE_SECS, MODELOP_STALE_SECS, reap_stale};
                let now = std::time::Instant::now();
                let reaped_tap = {
                    let mut m = s.blockop.lock().await;
                    reap_stale(&mut m, &live, now, BLOCKOP_STALE_SECS)
                };
                for p in reaped_tap {
                    eprintln!("[hygiene] reaped stale tap guard for {p}");
                }
                let reaped_model = {
                    let mut m = s.modelop.lock().await;
                    reap_stale(&mut m, &live, now, MODELOP_STALE_SECS)
                };
                for p in reaped_model {
                    eprintln!("[hygiene] reaped stale model guard for {p}");
                }
            }
            // Typing tasks for dead panes: ownership-checked stop, never raw
            // abort — a remint racing the tick keeps its task (a raw
            // extract_if+abort could kill a successor's task while its job
            // lives). Dead panes have no job by now (cancelled above), so
            // this still stops them; live/flapped panes are untouched.
            let drop_typing: Vec<String> = s
                .typing_tasks
                .lock()
                .await
                .keys()
                .filter(|p| !live.contains(*p))
                .cloned()
                .collect();
            for pane in &drop_typing {
                s.stop_typing_unless_owned(pane).await;
            }
            // Reply targets are NEVER pruned (not even for dead panes):
            // a DM reply to a corpse card must fail visibly via the
            // shell fallback, never silently reroute into the focused
            // live agent. Dead entries age out via the 512-cap overflow.
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::job::Job,
        state::{
            cancel::isolated_state,
            guard::{BLOCKOP_STALE_SECS, MODELOP_STALE_SECS},
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
        let mut cache = Some(HashSet::from(["live:p1".to_string(), "live:p2".to_string()]));
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
        let (s, _dir) = isolated_state();
        let job = Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("live:p1".into(), job);
        s.runwait.lock().await.insert((1, None), "w8".into());
        let mut cache = Some(HashSet::from(["live:p1".to_string()]));
        reap_orphans(&s, &mut cache).await;
        assert_eq!(
            s.runwait.lock().await.get(&(1, None)).map(String::as_str),
            Some("w8")
        );
    }
}
