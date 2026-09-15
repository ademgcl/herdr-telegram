//! Topic lifecycle: reopen/close/delete, shell badging, pin retirement,
//! mapping removal, and full-identity snapshot/restore (reset path).
//! Split from `manager` (300-line file limit).
use super::TopicManager;

impl TopicManager {
    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.last_title_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        self.creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(pane);
        self.storage.remove(pane)
    }

    /// F1: Reopen a closed forum topic (e.g. when agent transitions to working).
    pub async fn reopen_topic(&self, pane: &str) -> bool {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return true;
        };
        match self.tg.reopen_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    return true;
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
                self.remove_mapping(pane);
                println!("[topics] deleted topic #{thread} ({pane})");
                true
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping(pane);
                    return true;
                }
                eprintln!("[topics] delete topic #{thread} ({pane}) failed: {e}");
                false
            }
        }
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

    /// Restore a mapping wiped by `clear_all` (reset retry path):
    /// the Telegram topic survived, so re-sync reuses it with its tag,
    /// title and icon intact instead of minting a renamed duplicate or
    /// clobbering a user-customized icon.
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
}
