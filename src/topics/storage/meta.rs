//! Icon + recent-msg helpers: split from `storage` (300-line file limit).
use super::TopicStorage;

impl TopicStorage {
    pub fn get_icon(&self, pane: &str) -> Option<String> {
        self.lock().icons.get(pane).cloned()
    }

    pub fn set_icon(&self, pane: &str, icon: &str) {
        let mut s = self.lock();
        if s.icons.get(pane).map(|i| i.as_str()) != Some(icon) {
            s.icons.insert(pane.to_string(), icon.to_string());
            self.save(&s);
        }
    }

    /// Drop a user-cleared custom icon so the watchdog heals the kind
    /// glyph (a stale custom wedges kind flips forever via `is_bot_icon`).
    pub fn clear_icon(&self, pane: &str) {
        let mut s = self.lock();
        if s.icons.remove(pane).is_some() {
            self.save(&s);
        }
    }

    /// F6: Record last seen message ID for a pane (bounded to 3 recent msgs).
    pub fn record_msg(&self, pane: &str, mid: i64) {
        let mut s = self.lock();
        let list = s.last_msgs.entry(pane.to_string()).or_default();
        if list.last().copied() != Some(mid) {
            list.push(mid);
            if list.len() > 3 {
                list.remove(0);
            }
            self.save(&s);
        }
    }

    /// F6: Get up to 3 recent message IDs for this pane.
    pub fn get_recent_msgs(&self, pane: &str) -> Vec<i64> {
        self.lock().last_msgs.get(pane).cloned().unwrap_or_default()
    }

    /// F6: Drop stale mids after a reset migration (old topic deleted —
    /// a second reset must not re-copy corpses that now fail silently).
    pub fn clear_msgs(&self, pane: &str) {
        let mut s = self.lock();
        if s.last_msgs.remove(pane).is_some() {
            self.save(&s);
        }
    }
}
