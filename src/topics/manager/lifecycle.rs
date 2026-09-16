//! Topic lifecycle: reopen/close/delete, shell badging, pin retirement,
//! mapping removal, and full-identity snapshot/restore (reset path).
//! Split from `manager` (300-line file limit).
use super::TopicManager;
use crate::topics::names;

impl TopicManager {
    /// F1: Reopen a closed forum topic (e.g. when agent transitions to working).
    pub async fn reopen_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.reopen_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    // Human-deleted topic: drop the corpse so the next
                    // ensure recreates instead of reusing a dead thread.
                    // Compare-and-delete: never kill a fresh remint.
                    if self.remove_mapping_if_thread(pane, thread) {
                        println!("[topics] pruned deleted topic #{thread} ({pane}) on reopen");
                    }
                    return false;
                }
                eprintln!("[topics] reopen topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    /// Badge a live-but-agentless pane as shell: no title touch (titles
    /// sync 1:1 with herdr names), just ensures the topic.
    pub async fn mark_shell(&self, pane: &str) {
        self.sync_topic(pane, "shell", "?").await;
    }

    /// One-time cleanup of the retired pinned-status era: unpin leftovers.
    /// No-op once storage is clean.
    pub async fn retire_pins(&self) {
        let Some(forum) = self.forum_id else { return };
        for (pane, mid) in self.storage.take_pins() {
            println!("[topics] unpinning retired status pin #{mid} ({pane})");
            self.tg.unpin_msg(forum, mid).await;
        }
    }

    pub async fn close_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.close_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    // Corpse pruned inside (unlike the old caller-removes):
                    // the next ensure recreates instead of reusing dead.
                    self.remove_mapping_if_thread(pane, thread);
                    return true;
                }
                eprintln!("[topics] close topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    pub async fn delete_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.delete_forum_topic(forum, thread).await {
            Ok(()) => {
                // Compare-and-delete: a remint mid-RPC must survive.
                self.remove_mapping_if_thread(pane, thread);
                println!("[topics] deleted topic #{thread} ({pane})");
                true
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping_if_thread(pane, thread);
                    return true;
                }
                eprintln!("[topics] delete topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
    }

    /// Durable shell marker: creation tags (`sh<n>`) and the one-time
    /// context icon survive restarts, unlike `status` (empty at boot).
    /// Guards the already-shell retire decision when status is unknown —
    /// without it the first tick after a restart misclassifies a live
    /// shell long-run as a fresh agent→shell flip (ghost quit card +
    /// eaten shell intent). Strict `sh<n>` shape plus icon cross-check:
    /// in steady state an unknown future agent kind starting with "sh"
    /// keeps its agent icon and cannot collide. Residuals: a crash in
    /// the assign-tag→set-icon window leaves icon None + sh tag
    /// (transient false positive until the next icon sync); pre-guard
    /// "?"-stamped rows are not healed (the guard only stops new ones).
    pub fn is_shell_tagged(&self, pane: &str) -> bool {
        if self.storage.get_icon(pane).as_deref() == Some(names::context_icon_emoji_id("shell")) {
            return true;
        }
        // Legacy tag-only marker (icon never persisted for this pane).
        self.storage.get_icon(pane).is_none()
            && self.storage.get_tag(pane).is_some_and(|t| {
                t.starts_with("sh")
                    && !t[2..].is_empty()
                    && t[2..].chars().all(|c| c.is_ascii_digit())
            })
    }

    /// Snapshot a pane's full topic identity (reset survivor path).
    pub fn snapshot_identity(
        &self,
        pane: &str,
    ) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.storage.get_tag(pane),
            self.storage.get_title(pane),
            self.storage.get_icon(pane),
        )
    }

    /// Restore a mapping wiped by `clear_all` (reset retry path).
    /// Superseded by atomic `storage_clear_except`; kept for manual
    /// repair paths and tests.
    #[allow(dead_code)]
    pub fn restore_identity(
        &self,
        pane: String,
        thread: i64,
        tag: Option<String>,
        title: Option<String>,
        icon: Option<String>,
    ) {
        self.storage.insert(pane.clone(), thread);
        if let Some(t) = tag {
            self.storage.set_tag(&pane, &t);
        }
        if let Some(t) = title {
            self.storage.set_title(&pane, &t);
        }
        if let Some(i) = icon {
            self.storage.set_icon(&pane, &i);
        }
    }

    /// Full wipe (tests / manual repair). Reset uses atomic
    /// `storage_clear_except` instead.
    #[allow(dead_code)]
    pub fn clear_all(&self) {
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.storage.clear_all();
    }

    /// Atomic clear+restore for the reset survivor path (see storage).
    /// Preserves `creating` (in-flight guards stay): wiping it would let
    /// a concurrent ensure double-mint while the winner still holds its
    /// guard. Drains before the wipe cover the race instead.
    pub fn storage_clear_except(&self, kept: Vec<crate::topics::storage::KeptIdentity>) {
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.storage.clear_except(kept);
    }
}
