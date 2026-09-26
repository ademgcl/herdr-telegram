//! Global cancel: retire every watcher + intent in one pass. Split from
//! `cancel` (300-line file limit). Same LWW/CAS rules as the per-pane
//! paths; typing abort + done-stamps run AFTER CAS against keys that
//! were actually removed (early-snapshot abort kills a successor's
//! typing when its CAS loses).
use super::State;
use crate::jobs::job::Job;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};

/// Post-CAS stop set (pure, tested): actually-removed work ∪
/// early-owned panes that lost all work. The typing abort AND the
/// done-stamp loop both key off this — a successor that survived CAS
/// (still owned) stays out and keeps its typing task and stamp; aborting
/// or stamping from the early snapshot would kill the successor's
/// typing and gate a live turn's DM spontaneous.
pub(crate) fn global_cancel_stop(
    removed: &std::collections::HashSet<String>,
    early_owned: &std::collections::HashSet<String>,
    still_owned: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut stop = removed.clone();
    for p in early_owned {
        if !still_owned.contains(p) {
            stop.insert(p.clone());
        }
    }
    stop
}

impl State {
    pub async fn cancel_all_jobs(self: &Arc<Self>) -> usize {
        // Narrow critical sections: take each map, drop its guard, then
        // act — never hold typing_tasks across the jobs/pending/waiter
        // locks (a future inverse nesting would deadlock, and every
        // typing start/stop blocks for the whole global cancel).
        // 1:1 working↔typing: abort only panes that owned cancellable
        // work (job or intent) — a global /cancel must not darken a
        // spontaneous working bystander with neither (its next heal is
        // otherwise the 60s watchdog while it keeps working).
        // Early owned-set is the typing/stamp *candidate* set only:
        // typing abort + done-stamps run AFTER the CAS below against
        // keys that were actually removed (or early panes that lost all
        // work) — aborting from the early snapshot kills a successor's
        // typing when its CAS loses, and stamps a pane still owned.
        let early_owned: std::collections::HashSet<String> = {
            let mut s: std::collections::HashSet<String> =
                self.jobs.lock().await.keys().cloned().collect();
            s.extend(self.pending.lock().await.keys().cloned());
            s
        };
        // Last-writer-wins (per-pane parity): an enqueue landing between
        // the snapshots above and the clears below owns its intent — a
        // blind take-all + clear-all would wipe a successfully submitted
        // prompt with no reply ever arriving. Same-Arc reuse bumps the
        // epoch in place, so the snapshot pairs each Arc with its epoch.
        let live_jobs: HashMap<String, (Arc<Job>, u64)> = {
            let map = self.jobs.lock().await;
            map.iter()
                .map(|(p, j)| (p.clone(), (j.clone(), j.epoch.load(Ordering::Relaxed))))
                .collect()
        };
        let live_pending = self.pending.lock().await.clone();
        let jobs: HashMap<String, Arc<Job>> = {
            let mut map = self.jobs.lock().await;
            let mut taken = HashMap::new();
            for (p, (j, epoch)) in &live_jobs {
                let same = map
                    .get(p)
                    .map(|c| Arc::ptr_eq(c, j) && c.epoch.load(Ordering::Relaxed) == *epoch)
                    .unwrap_or(false);
                if same && let Some(removed) = map.remove(p) {
                    taken.insert(p.clone(), removed);
                }
            }
            taken
        };
        let cleared_pending: Vec<String> = {
            let mut map = self.pending.lock().await;
            let mut snap = live_pending.clone();
            snap.retain(|p, pp| {
                // Stamp-pinned (global_cancel_clears parity): full
                // equality — text equality alone would wipe an identical
                // resubmit's fresh intent (started_unix pins it).
                super::retire::global_cancel_clears(map.get(p), pp)
            });
            let cleared: Vec<String> = snap.keys().cloned().collect();
            for p in &cleared {
                map.remove(p);
            }
            let remaining = map.clone();
            drop(map);
            crate::jobs::persist::save_file(&crate::jobs::persist::store_path(), &remaining);
            cleared
        };
        // Actually removed this pass ∪ early-owned panes that lost all
        // work (orphan typing/stamp parity — a successor that survived
        // CAS is still owned and must keep both). Ownership only for
        // early panes not already removed (removed ⇒ stop regardless).
        let mut removed: std::collections::HashSet<String> = jobs.keys().cloned().collect();
        removed.extend(cleared_pending.iter().cloned());
        let mut still_owned: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in &early_owned {
            if removed.contains(p) {
                continue;
            }
            if self.jobs.lock().await.contains_key(p) || self.pending.lock().await.contains_key(p) {
                still_owned.insert(p.clone());
            }
        }
        let stop = global_cancel_stop(&removed, &early_owned, &still_owned);
        // Typing abort only after CAS: ownership re-checked below so a
        // submit landing in the window re-mints instead of staying dark.
        let doomed: Vec<tokio::task::JoinHandle<()>> = {
            let mut tasks = self.typing_tasks.lock().await;
            let kill: Vec<String> = tasks
                .keys()
                .filter(|p| stop.contains(*p))
                .cloned()
                .collect();
            kill.into_iter().filter_map(|p| tasks.remove(&p)).collect()
        };
        for handle in doomed {
            handle.abort();
        }
        for pane in &stop {
            if self.job_live(pane).await || self.pending.lock().await.contains_key(pane) {
                self.start_typing(pane).await;
            }
        }
        // Global cancel retires everything: armed input waiters and
        // settle debounces die with the jobs, or the next message/card
        // would serve a cancelled world.
        self.typewait.lock().await.clear();
        self.keywait.lock().await.clear();
        self.runwait.lock().await.clear();
        self.debounce.lock().await.clear();
        // Fresh episodes everywhere after a global cancel (see
        // cancel_jobs_for for the per-pane reason).
        self.clear_all_limit_episodes().await;
        // DM done-stamps (cancel_jobs_for parity): in-flight DM
        // spontaneous owns no arm — without this it posts past the
        // global cancel via its done-after check. Keys are the post-CAS
        // `stop` set, never the early snapshot (a pane whose successor
        // survived must not be stamped).
        {
            let now = std::time::Instant::now();
            let mut done = self.last_done.lock().await;
            for pane in &stop {
                done.insert(pane.clone(), now);
            }
        }
        let count = jobs.len();
        for job in jobs.values() {
            job.mark_stopped();
            // Same epoch-bump as cancel_jobs_for: in-flight posts abort.
            job.epoch.fetch_add(1, Ordering::Relaxed);
            job.cancel.notify_waiters();
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_state::isolated_state_with_forum;

    fn keys(items: &[&str]) -> std::collections::HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_post_cas_stop_set_successor_keeps_typing_and_stamp() {
        // The post-CAS contract: removed work always stops; an
        // early-owned successor that survived CAS (still owned) stays
        // OUT (keeps typing + escapes the stamp); an early pane that
        // lost all work stops; a never-owned bystander never enters.
        let removed = keys(&["t:a"]);
        let early = keys(&["t:a", "t:b", "t:c"]);
        let still_owned = keys(&["t:b"]); // b's successor won CAS
        let stop = global_cancel_stop(&removed, &early, &still_owned);
        assert!(stop.contains("t:a"), "removed work must stop");
        assert!(
            !stop.contains("t:b"),
            "surviving successor must keep typing/stamp"
        );
        assert!(stop.contains("t:c"), "early + unowned must stop");
        assert!(!stop.contains("t:by"), "bystander never enters stop");
        // Removal wins over ownership (regained slot after clear still stops).
        let stop = global_cancel_stop(&removed, &early, &keys(&["t:b", "t:a"]));
        assert!(stop.contains("t:a"));
    }

    #[tokio::test]
    async fn test_cancel_all_post_cas_aborts_owned_typing_spares_bystander() {
        // End-to-end stop-set keys: owned pane → typing aborted +
        // done-stamped; spontaneous bystander (typing, no work) →
        // neither (a global /cancel must not darken it).
        let (s, _dir) = isolated_state_with_forum(Some(7));
        let job = Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job.clone());
        s.start_typing("t:p1").await;
        s.start_typing("t:by").await;
        {
            let tasks = s.typing_tasks.lock().await;
            assert!(tasks.contains_key("t:p1") && tasks.contains_key("t:by"));
        }
        s.pending.lock().await.insert(
            "t:sh".into(),
            crate::jobs::persist::PendingPrompt {
                chat: 1,
                thread: None,
                prompt: "hi".into(),
                started_unix: 7,
            },
        );
        s.start_typing("t:sh").await;
        assert_eq!(s.cancel_all_jobs().await, 1);
        {
            let tasks = s.typing_tasks.lock().await;
            assert!(!tasks.contains_key("t:p1"), "owned typing must abort");
            assert!(
                !tasks.contains_key("t:sh"),
                "pending-only owned typing must abort"
            );
            assert!(tasks.contains_key("t:by"), "bystander typing must survive");
        }
        let done = s.last_done.lock().await;
        assert!(done.contains_key("t:p1"), "job pane stamped");
        assert!(done.contains_key("t:sh"), "pending-only pane stamped");
        assert!(!done.contains_key("t:by"), "bystander never stamped");
        assert!(job.is_stopped());
        assert!(!s.pending.lock().await.contains_key("t:sh"));
    }
}
