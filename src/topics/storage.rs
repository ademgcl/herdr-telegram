use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::PathBuf,
    sync::Mutex,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Store {
    #[serde(default)]
    topics: HashMap<String, i64>,
    #[serde(default)]
    unread: HashSet<String>,
    /// Stable short tags per pane ("o2") — survive restarts so topic
    /// names never reshuffle. Missing in old files → default empty.
    #[serde(default)]
    tags: HashMap<String, String>,
    /// Pinned live-status message per pane (pane → message id).
    #[serde(default)]
    pins: HashMap<String, i64>,
}

pub struct TopicStorage {
    file_path: PathBuf,
    store: Mutex<Store>,
}

impl TopicStorage {
    pub fn new() -> Self {
        // Keep state self-contained next to the bot; migrate legacy XDG file once
        let path = PathBuf::from("topics.state");
        let home = env::var("HOME").unwrap_or_default();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/topics.json"));
        if !path.exists()
            && legacy.exists()
            && std::fs::copy(&legacy, &path).is_ok()
        {
            println!("[topics] migrated {} → topics.state", legacy.display());
        }
        Self::at(path)
    }

    pub(crate) fn at(file_path: PathBuf) -> Self {
        let store = Self::read_from_disk(&file_path);
        Self {
            file_path,
            store: Mutex::new(store),
        }
    }

    fn read_from_disk(path: &PathBuf) -> Store {
        let Ok(txt) = fs::read_to_string(path) else { return Store::default() };
        // Current format
        if let Ok(s) = serde_json::from_str::<Store>(&txt) {
            return s;
        }
        // Legacy flat map {pane: thread}
        serde_json::from_str::<HashMap<String, i64>>(&txt)
            .map(|topics| Store {
                topics,
                unread: HashSet::new(),
                tags: HashMap::new(),
                pins: HashMap::new(),
            })
            .unwrap_or_default()
    }

    fn save(&self, s: &Store) {
        if let Some(parent) = self.file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(s) {
            let _ = fs::write(&self.file_path, json);
        }
    }

    pub fn get_thread(&self, pane: &str) -> Option<i64> {
        self.store.lock().unwrap().topics.get(pane).copied()
    }

    pub fn get_pane(&self, thread: i64) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .topics
            .iter()
            .find(|(_, t)| **t == thread)
            .map(|(p, _)| p.clone())
    }

    pub fn insert(&self, pane: String, thread: i64) {
        let mut s = self.store.lock().unwrap();
        s.topics.insert(pane, thread);
        self.save(&s);
    }

    pub fn remove(&self, pane: &str) -> Option<i64> {
        let mut s = self.store.lock().unwrap();
        let prev = s.topics.remove(pane);
        s.unread.remove(pane);
        let untagged = s.tags.remove(pane);
        let unpinned = s.pins.remove(pane);
        if prev.is_some() || untagged.is_some() || unpinned.is_some() {
            self.save(&s);
        }
        prev
    }

    /// Get-or-assign this pane's stable tag, atomically under one lock so
    /// concurrent topic creations never hand out the same tag twice.
    pub fn assign_tag(&self, pane: &str, kind: &str) -> String {
        let mut s = self.store.lock().unwrap();
        if let Some(t) = s.tags.get(pane) {
            return t.clone();
        }
        let taken: Vec<String> = s.tags.values().cloned().collect();
        let tag = super::names::assign(&taken, kind);
        s.tags.insert(pane.to_string(), tag.clone());
        self.save(&s);
        tag
    }

    /// Drain all pinned-status leftovers (retired era) — persisted so the
    /// cleanup runs exactly once across restarts.
    pub fn take_pins(&self) -> HashMap<String, i64> {
        let mut s = self.store.lock().unwrap();
        let pins = std::mem::take(&mut s.pins);
        if !pins.is_empty() {
            self.save(&s);
        }
        pins
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.store.lock().unwrap().topics.clone()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_roundtrip_and_migration() {
        let legacy = r#"{"w1:p1": 42}"#;
        let migrated: Store = serde_json::from_str(legacy)
            .or_else(|_| serde_json::from_str::<HashMap<String, i64>>(legacy).map(|m| Store { topics: m, unread: HashSet::new(), tags: HashMap::new(), pins: HashMap::new() }))
            .unwrap();
        assert_eq!(migrated.topics.get("w1:p1"), Some(&42));
        assert!(migrated.unread.is_empty());

        let s = Store::default();
        assert!(!s.unread.contains("x"));
    }

    #[test]
    fn test_persistence_across_reopen() {
        let st = TopicStorage::at(PathBuf::from(format!(
            "/tmp/herdr-tg-test-topics-{}.json",
            std::process::id()
        )));
        st.insert("w9:p9".into(), 77);
        let re = TopicStorage::at(st.file_path.clone());
        assert_eq!(re.get_thread("w9:p9"), Some(77));
        let _ = std::fs::remove_file(&st.file_path);
    }

    #[test]
    fn test_tags_stable_and_reassigned() {
        let st = TopicStorage::at(PathBuf::from(format!(
            "/tmp/herdr-tg-test-tags-{}.json",
            std::process::id()
        )));
        assert_eq!(st.assign_tag("w1:p1", "opencode"), "o1");
        assert_eq!(st.assign_tag("w1:p1", "opencode"), "o1");
        assert_eq!(st.assign_tag("w1:p2", "opencode"), "o2");
        assert_eq!(st.assign_tag("w2:p1", "claude"), "c1");
        // Persisted across reopen; freed tags are refilled.
        let re = TopicStorage::at(st.file_path.clone());
        assert_eq!(re.assign_tag("w1:p2", "opencode"), "o2");
        re.remove("w1:p1");
        assert_eq!(re.assign_tag("w3:p9", "opencode"), "o1");
        let _ = std::fs::remove_file(&st.file_path);
    }
}
