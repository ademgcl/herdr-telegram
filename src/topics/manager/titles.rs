//! 1:1 pane↔topic title sync (both directions) plus stable short tags.
//! Split from `manager` (300-line file limit).
use super::TopicManager;
impl TopicManager {
    /// Stable short tag for this pane (`o2`) — backs the friendly
    /// default title for unlabeled panes.
    pub fn tag_for(&self, pane: &str, kind: &str) -> String {
        self.storage.assign_tag(pane, kind)
    }

    /// Last synced 1:1 title for this pane (herdr label or pane id).
    pub fn topic_title(&self, pane: &str) -> Option<String> {
        self.storage.get_title(pane)
    }

    /// Memory-only last seen kind per pane (kind-flip detector, no disk).
    pub fn note_kind(&self, pane: &str, kind: &str) {
        self.last_kind
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pane.to_string(), kind.to_string());
    }

    /// Live-only retain for the kind memory (reap parity with the
    /// per-pane maps in hygiene): mapping-less dead panes (shells,
    /// never-minted topics) never pass `remove_mapping_if_thread`, so
    /// without this every dead pane id leaks one entry forever.
    pub fn prune_kinds(&self, live: &std::collections::HashSet<String>) {
        self.last_kind
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p, _| live.contains(p));
    }

    /// CAS record of a title the telegram side already shows (native
    /// rename): no API call, just the echo loop-guard. Thread-checked so
    /// a remint between resolve and store never gains a stale title.
    /// Returns stored or not.
    pub fn note_title_if_thread(&self, pane: &str, thread: i64, title: &str) -> bool {
        if !self.storage.set_title_if_thread(pane, thread, title) {
            return false;
        }
        true
    }

    /// herdr→telegram half: rename the topic when the pane's desired
    /// title drifted. Silent; stores only on success so failures retry
    /// on the next watchdog tick. No time debounce: `stored==desired`
    /// already suppresses echoes, and a time gate would defer a real
    /// herdr rename behind a Telegram adopt (late reflection). Returns
    /// the pane when the mapping was pruned (caller retires its dialog
    /// generation — a same-content blocked dialog must repost, not stay
    /// silent on a stale sig).
    pub async fn sync_title(&self, pane: &str, desired: &str) -> Option<String> {
        if self.storage.get_title(pane).as_deref() == Some(desired) {
            return None;
        }
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return None;
        };
        match self.tg.set_topic_title(forum, thread, desired).await {
            Ok(()) => {
                // Atomic compare-and-set: a remint between snapshot and
                // store must not gain an orphan title.
                if self.storage.set_title_if_thread(pane, thread, desired) {
                    println!("[topics] renamed topic #{thread} ({pane}) to {desired:?}");
                }
                None
            }
            Err(e) => {
                if crate::telegram::topic_gone(&e.to_string()) {
                    // Compare-and-delete: a stale rename must never kill
                    // a fresh mapping minted after the snapshot. `gone`
                    // (not just `missing`): a kick/chat-delete must prune
                    // here too, not linger until the probe ring (probe
                    // parity — one source per prune verdict).
                    if self.remove_mapping_if_thread(pane, thread) {
                        println!("[topics] pruned missing topic #{thread} ({pane})");
                        return Some(pane.to_string());
                    }
                    None
                } else if crate::telegram::topic_not_modified(&e.to_string()) {
                    // Already showing it — converged, store and stay quiet
                    // instead of retry-spamming every watchdog tick.
                    let _ = self.storage.set_title_if_thread(pane, thread, desired);
                    None
                } else {
                    eprintln!(
                        "[topics] rename topic #{thread} ({pane}) failed: {}",
                        self.tg.redact(&e.to_string())
                    );
                    None
                }
            }
        }
    }

    /// Round-robin liveness probe: re-assert mappings' stored titles
    /// per watchdog tick so a human-deleted topic prunes within ≤60s.
    /// Converged titles otherwise never fire an RPC, so a deleted topic
    /// would dangle — the probe answers TOPIC_ID_INVALID → prune, and
    /// the next ensure recreates. Budget scales with map size (all
    /// mappings each tick): N RPCs per 60s stays well under limits for
    /// realistic hosts and keeps herdr→tg ≤60s for any N. Skipped while
    /// resetting (would fight identity restore). Returns pruned panes
    /// for dialog retire.
    pub async fn probe_deleted(&self) -> Vec<String> {
        if crate::handlers::reset::is_resetting() {
            return Vec::new();
        }
        let Some(forum) = self.forum_id else {
            return Vec::new();
        };
        let n = self.storage.all_mappings().len().max(3);
        let mut pruned = Vec::new();
        for _ in 0..n {
            if let Some(p) = self.probe_next(forum).await {
                pruned.push(p);
            }
        }
        pruned
    }

    /// Probe a single mapping (one RPC): the ring cursor advances per
    /// call so consecutive calls walk the map. Split for the scaled
    /// per-tick loop above. Returns the pane when pruned.
    async fn probe_next(&self, forum: i64) -> Option<String> {
        let mappings = self.storage.all_mappings();
        if mappings.is_empty() {
            return None;
        }
        let mut panes: Vec<String> = mappings.keys().cloned().collect();
        panes.sort();
        // Single cursor lock: peek + advance together (no concurrent
        // double-probe/skip). Title-less still advances — its reopen
        // fallback heals that rotation instead of starving the ring.
        let (pane, thread) = {
            let mut c = self.probe_cursor.lock().unwrap_or_else(|e| e.into_inner());
            let idx = *c % panes.len();
            let pane = panes[idx].clone();
            *c = idx.wrapping_add(1);
            let thread = match mappings.get(&pane).copied() {
                Some(t) => t,
                None => return None,
            };
            (pane, thread)
        };
        let Some(title) = self.storage.get_title(&pane) else {
            // Title-less (legacy flat migration): heal via reopen (no
            // title needed) — missing prunes, live stays. A dropped
            // mapping (here or concurrently) retires the dialog via the
            // returned pane.
            if self.reopen_topic(&pane).await.is_some() || self.storage.get_thread(&pane).is_none()
            {
                println!("[topics] probe healed title-less corpse ({pane})");
                return Some(pane);
            }
            eprintln!("[topics] probe: title-less mapping ({pane}) kept");
            return None;
        };
        match self.tg.set_topic_title(forum, thread, &title).await {
            Ok(()) => None,
            Err(e) if crate::telegram::topic_not_modified(&e.to_string()) => None,
            Err(e) if crate::telegram::topic_gone(&e.to_string()) => {
                if self.remove_mapping_if_thread(&pane, thread) {
                    println!("[topics] probe pruned deleted topic #{thread} ({pane})");
                    return Some(pane);
                }
                None
            }
            Err(e) => {
                eprintln!(
                    "[topics] probe topic #{thread} ({pane}) failed: {}",
                    self.tg.redact(&e.to_string())
                );
                None
            }
        }
    }
}
