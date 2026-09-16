//! 1:1 pane↔topic title sync (both directions) plus stable short tags.
//! Split from `manager` (300-line file limit).
use super::TopicManager;
use std::time::Instant;
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

    /// CAS record of a title the telegram side already shows (native
    /// rename): no API call, just the echo loop-guard. Thread-checked so
    /// a remint between resolve and store never gains a stale title.
    /// Returns stored or not.
    pub fn note_title_if_thread(&self, pane: &str, thread: i64, title: &str) -> bool {
        if !self.storage.set_title_if_thread(pane, thread, title) {
            return false;
        }
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pane.to_string(), Instant::now());
        true
    }

    /// herdr→telegram half: rename the topic when the pane's desired
    /// title drifted. Silent; stores only on success so failures retry
    /// on the next watchdog tick.
    pub async fn sync_title(&self, pane: &str, desired: &str) {
        if self.storage.get_title(pane).as_deref() == Some(desired) {
            return;
        }
        if let Some(t) = self
            .last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(pane)
            && t.elapsed() < std::time::Duration::from_secs(5)
        {
            println!("[topics] skip rename {pane}: recent title write");
            return;
        }
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return;
        };
        match self.tg.set_topic_title(forum, thread, desired).await {
            Ok(()) => {
                // Atomic compare-and-set: a remint between snapshot and
                // store must not gain an orphan title.
                if self.storage.set_title_if_thread(pane, thread, desired) {
                    println!("[topics] renamed topic #{thread} ({pane}) to {desired:?}");
                    self.last_title_write
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(pane.to_string(), Instant::now());
                }
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    // Compare-and-delete: a stale rename must never kill
                    // a fresh mapping minted after the snapshot.
                    if self.remove_mapping_if_thread(pane, thread) {
                        println!("[topics] pruned missing topic #{thread} ({pane})");
                    }
                } else if crate::telegram::topic_not_modified(&e.to_string()) {
                    // Already showing it — converged, store and stay quiet
                    // instead of retry-spamming every watchdog tick.
                    // Same atomic guard as the Ok path.
                    if self.storage.set_title_if_thread(pane, thread, desired) {
                        self.last_title_write
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(pane.to_string(), Instant::now());
                    }
                } else {
                    eprintln!("[topics] rename topic #{thread} ({pane}) failed: {e}");
                }
            }
        }
    }

    /// Round-robin liveness probe: re-assert ONE mapping's stored title
    /// per watchdog tick (1 RPC). Converged titles otherwise never fire
    /// an RPC, so a human-deleted topic would dangle forever — the probe
    /// answers TOPIC_ID_INVALID → prune, and the next ensure recreates.
    /// Skipped while resetting (would fight identity restore).
    pub async fn probe_deleted(&self) {
        if crate::handlers::reset::is_resetting() {
            return;
        }
        let Some(forum) = self.forum_id else {
            return;
        };
        let mappings = self.storage.all_mappings();
        if mappings.is_empty() {
            return;
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
                None => return,
            };
            (pane, thread)
        };
        let Some(title) = self.storage.get_title(&pane) else {
            // Title-less (legacy flat migration): heal via reopen (no
            // title needed) — missing prunes, live stays.
            if !self.reopen_topic(&pane).await && self.storage.get_thread(&pane).is_none() {
                println!("[topics] probe healed title-less corpse ({pane})");
            } else {
                eprintln!("[topics] probe: title-less mapping ({pane}) kept");
            }
            return;
        };
        match self.tg.set_topic_title(forum, thread, &title).await {
            Ok(()) => {}
            Err(e) if crate::telegram::topic_not_modified(&e.to_string()) => {}
            Err(e) if crate::telegram::topic_missing(&e.to_string()) => {
                if self.remove_mapping_if_thread(&pane, thread) {
                    println!("[topics] probe pruned deleted topic #{thread} ({pane})");
                }
            }
            Err(e) => {
                eprintln!("[topics] probe topic #{thread} ({pane}) failed: {e}");
            }
        }
    }
}
