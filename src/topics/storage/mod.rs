//! Durable topic identity (pane→thread/tag/title/icon/card): JSON file
//! with `.prev` backup, mutex-guarded in memory. Split into `disk` + `tests`.
//! (`card` persists under the legacy `pins` key — same ids, never pinned.)
use std::{collections::HashMap, env, path::PathBuf, sync::Mutex};

mod disk;
#[cfg(test)]
mod tests;

use disk::Store;

pub struct TopicStorage {
    file_path: PathBuf,
    store: Mutex<Store>,
}

impl TopicStorage {
    pub fn new() -> Self {
        let path = crate::state::state_dir().join("topics.state");
        let home = env::var("HOME").unwrap_or_default();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/topics.json"));
        if !path.exists() && legacy.exists() && std::fs::copy(&legacy, &path).is_ok() {
            println!(
                "[topics] migrated {} → topics.state",
                crate::types::collapse_home(&legacy.display().to_string(), &home)
            );
        }
        Self::at(path)
    }

    pub(crate) fn at(file_path: PathBuf) -> Self {
        let store = disk::read_store(&file_path);
        Self {
            file_path,
            store: Mutex::new(store),
        }
    }

    fn save(&self, s: &Store) {
        disk::write_store(&self.file_path, s, true);
    }

    fn save_no_backup(&self, s: &Store) {
        disk::write_store(&self.file_path, s, false);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_thread(&self, pane: &str) -> Option<i64> {
        self.lock().topics.get(pane).copied()
    }

    pub fn get_pane(&self, thread: i64) -> Option<String> {
        self.lock()
            .topics
            .iter()
            .find(|(_, t)| **t == thread)
            .map(|(p, _)| p.clone())
    }

    /// Test-only surface (roundtrip + wipe tests pin the disk format).
    #[allow(dead_code)]
    pub fn insert(&self, pane: String, thread: i64) {
        let mut s = self.lock();
        s.topics.insert(pane, thread);
        self.save(&s);
    }

    /// Atomic thread+title insert: one lock, one save — no crash window
    /// leaving a title-less mapping the probe would skip forever.
    pub fn insert_with_title(&self, pane: String, thread: i64, title: &str) {
        let mut s = self.lock();
        s.topics.insert(pane.clone(), thread);
        s.titles.insert(pane, title.to_string());
        self.save(&s);
    }

    /// Full remove (tests / manual repair). Hot paths use atomic
    /// `remove_if_thread` instead.
    #[allow(dead_code)]
    pub fn remove(&self, pane: &str) -> Option<i64> {
        let mut s = self.lock();
        let prev = s.topics.remove(pane);
        s.unread.remove(pane);
        let a = s.tags.remove(pane);
        let b = s.titles.remove(pane);
        let c = s.pins.remove(pane);
        let d = s.icons.remove(pane);
        let e = s.last_msgs.remove(pane);
        if prev.is_some() || a.is_some() || b.is_some() || c.is_some() || d.is_some() || e.is_some()
        {
            self.save(&s);
        }
        prev
    }

    /// Single-lock compare-and-delete: check thread + remove under ONE
    /// guard — no interleave can wipe a fresh remint between check and
    /// remove. Returns true when something was pruned.
    pub fn remove_if_thread(&self, pane: &str, thread: i64) -> bool {
        let mut s = self.lock();
        if s.topics.get(pane) != Some(&thread) {
            return false;
        }
        s.topics.remove(pane);
        s.unread.remove(pane);
        s.tags.remove(pane);
        s.titles.remove(pane);
        s.pins.remove(pane);
        s.icons.remove(pane);
        s.last_msgs.remove(pane);
        self.save(&s);
        true
    }

    /// Roll back a tag leaked by a failed create (no thread ever minted).
    pub fn remove_tag_if_threadless(&self, pane: &str) {
        let mut s = self.lock();
        if s.topics.contains_key(pane) {
            return;
        }
        if s.tags.remove(pane).is_some() {
            self.save(&s);
        }
    }

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

    pub fn get_tag(&self, pane: &str) -> Option<String> {
        self.lock().tags.get(pane).cloned()
    }

    pub fn get_title(&self, pane: &str) -> Option<String> {
        self.lock().titles.get(pane).cloned()
    }

    /// Test-only surface (roundtrip + wipe tests pin the disk format).
    #[allow(dead_code)]
    pub fn set_title(&self, pane: &str, title: &str) {
        let mut s = self.lock();
        if s.titles.get(pane).map(|t| t.as_str()) == Some(title) {
            return;
        }
        s.titles.insert(pane.to_string(), title.to_string());
        self.save(&s);
    }

    /// Atomic compare-and-set title: stores only when the thread still
    /// matches (no orphan title on a fresh remint). Returns stored or not.
    pub fn set_title_if_thread(&self, pane: &str, thread: i64, title: &str) -> bool {
        let mut s = self.lock();
        if s.topics.get(pane) != Some(&thread) {
            return false;
        }
        if s.titles.get(pane).map(|t| t.as_str()) == Some(title) {
            return true;
        }
        s.titles.insert(pane.to_string(), title.to_string());
        self.save(&s);
        true
    }

    /// Get-or-assign stable tag atomically: concurrent creates never
    /// hand out the same tag twice.
    pub fn assign_tag(&self, pane: &str, kind: &str) -> String {
        let mut s = self.lock();
        if let Some(t) = s.tags.get(pane) {
            return t.clone();
        }
        let taken: Vec<String> = s.tags.values().cloned().collect();
        let tag = super::names::assign(&taken, kind);
        s.tags.insert(pane.to_string(), tag.clone());
        self.save(&s);
        tag
    }

    /// Identity-card message id per pane (tracked for status edits —
    /// nothing is ever pinned). Legacy `pins` key kept for disk compat.
    pub fn get_pin(&self, pane: &str) -> Option<i64> {
        self.lock().pins.get(pane).copied()
    }

    pub fn set_pin(&self, pane: &str, mid: i64) {
        let mut s = self.lock();
        if s.pins.get(pane).copied() == Some(mid) {
            return;
        }
        s.pins.insert(pane.to_string(), mid);
        self.save(&s);
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

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.lock().topics.clone()
    }

    /// Test-only surface (wipe test pins the empty-store format).
    #[allow(dead_code)]
    pub fn clear_all(&self) {
        let mut s = self.lock();
        s.topics.clear();
        s.unread.clear();
        s.tags.clear();
        s.titles.clear();
        s.pins.clear();
        s.icons.clear();
        s.last_msgs.clear();
        // No backup: empty intermediate must not clobber last-good.
        self.save_no_backup(&s);
    }
}

// Re-export for tests using prev-path convention.
