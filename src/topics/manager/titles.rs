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

    /// Record a title the telegram side already shows (native rename):
    /// no API call, just the echo loop-guard.
    pub fn note_title(&self, pane: &str, title: &str) {
        self.storage.set_title(pane, title);
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pane.to_string(), Instant::now());
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
                println!("[topics] renamed topic #{thread} ({pane}) to {desired:?}");
                self.storage.set_title(pane, desired);
                self.last_title_write
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(pane.to_string(), Instant::now());
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping(pane);
                    println!("[topics] pruned missing topic #{thread} ({pane})");
                } else if crate::telegram::topic_not_modified(&e.to_string()) {
                    // Already showing it — converged, store and stay quiet
                    // instead of retry-spamming every watchdog tick.
                    self.storage.set_title(pane, desired);
                    self.last_title_write
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(pane.to_string(), Instant::now());
                } else {
                    eprintln!("[topics] rename topic #{thread} ({pane}) failed: {e}");
                }
            }
        }
    }
}
