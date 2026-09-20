//! Durable topic identity (pane→thread/tag/title/icon/card): JSON file
//! with `.prev` backup, mutex-guarded in memory. Split into `disk` + `tests`.
//! (`card` persists under the legacy `pins` key — same ids, never pinned.)
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

mod disk;
mod meta;
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
        let home = crate::types::home_dir();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/topics.json"));
        if !path.exists() && legacy.exists() && std::fs::copy(&legacy, &path).is_ok() {
            crate::types::chmod_private(&path);
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

    /// One-time heal for orphans: `last_msgs` without a mapping
    /// is write-only (only topic resets read it). Called at boot in
    /// every mode — DM→forum switches orphan the same way.
    pub fn prune_orphan_msgs(&self) {
        let mut s = self.lock();
        let orphans: Vec<String> = s
            .last_msgs
            .keys()
            .filter(|p| !s.topics.contains_key(*p))
            .cloned()
            .collect();
        if !orphans.is_empty() {
            for p in orphans {
                s.last_msgs.remove(&p);
            }
            self.save(&s);
        }
    }

    fn save(&self, s: &Store) {
        disk::write_store(&self.file_path, s, true);
    }

    #[cfg(test)]
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
    #[cfg(test)]
    pub fn insert(&self, pane: String, thread: i64) {
        let mut s = self.lock();
        s.topics.insert(pane, thread);
        self.save(&s);
    }

    /// Atomic thread+title insert: one lock, one save — no crash window
    /// leaving a title-less mapping the probe would skip forever. A
    /// changed thread also drops the pin (a remint's fresh card mints
    /// its own; a failed send must not leave edits aimed at a deleted
    /// message while the new topic starves).
    pub fn insert_with_title(&self, pane: String, thread: i64, title: &str) {
        let mut s = self.lock();
        if s.topics.get(&pane) != Some(&thread) {
            s.pins.remove(&pane);
        }
        s.topics.insert(pane.clone(), thread);
        s.titles.insert(pane, title.to_string());
        self.save(&s);
    }

    /// Full remove for tests. Hot paths use atomic
    /// `remove_if_thread` instead.
    #[cfg(test)]
    pub fn remove(&self, pane: &str) -> Option<i64> {
        let mut s = self.lock();
        let prev = s.topics.remove(pane);
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

    pub fn get_tag(&self, pane: &str) -> Option<String> {
        self.lock().tags.get(pane).cloned()
    }

    pub fn get_title(&self, pane: &str) -> Option<String> {
        self.lock().titles.get(pane).cloned()
    }

    /// Test-only surface (roundtrip + wipe tests pin the disk format).
    #[cfg(test)]
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

    /// Test-only pin store (prod paths use the CAS guard below).
    #[cfg(test)]
    pub fn set_pin(&self, pane: &str, mid: i64) {
        let mut s = self.lock();
        if s.pins.get(pane).copied() == Some(mid) {
            return;
        }
        s.pins.insert(pane.to_string(), mid);
        self.save(&s);
    }

    /// Atomic compare-and-set pin: stores only when the thread still
    /// matches (no orphan pin on a fresh remint — future edits would
    /// target a deleted message). Returns stored or not.
    pub fn set_pin_if_thread(&self, pane: &str, thread: i64, mid: i64) -> bool {
        let mut s = self.lock();
        if s.topics.get(pane) != Some(&thread) {
            return false;
        }
        if s.pins.get(pane).copied() == Some(mid) {
            return true;
        }
        s.pins.insert(pane.to_string(), mid);
        self.save(&s);
        true
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.lock().topics.clone()
    }

    /// Test-only surface (wipe test pins the empty-store format).
    #[cfg(test)]
    pub fn clear_all(&self) {
        let mut s = self.lock();
        s.topics.clear();
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
