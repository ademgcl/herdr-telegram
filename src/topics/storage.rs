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
        if prev.is_some() {
            self.save(&s);
        }
        prev
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.store.lock().unwrap().topics.clone()
    }

    pub fn is_unread(&self, pane: &str) -> bool {
        self.store.lock().unwrap().unread.contains(pane)
    }

    /// Returns true if this transitioned the pane to unread.
    pub fn mark_unread(&self, pane: &str) -> bool {
        let mut s = self.store.lock().unwrap();
        if s.unread.insert(pane.to_string()) {
            self.save(&s);
            return true;
        }
        false
    }

    /// Returns true if this transitioned the pane to read.
    pub fn mark_read(&self, pane: &str) -> bool {
        let mut s = self.store.lock().unwrap();
        if s.unread.remove(pane) {
            self.save(&s);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_roundtrip_and_migration() {
        let legacy = r#"{"w1:p1": 42}"#;
        let migrated: Store = serde_json::from_str(legacy)
            .or_else(|_| serde_json::from_str::<HashMap<String, i64>>(legacy).map(|m| Store { topics: m, unread: HashSet::new() }))
            .unwrap();
        assert_eq!(migrated.topics.get("w1:p1"), Some(&42));
        assert!(migrated.unread.is_empty());

        let s = Store::default();
        assert!(!s.unread.contains("x"));
    }

    #[test]
    fn test_unread_transitions() {
        let st = TopicStorage::at(PathBuf::from(format!(
            "/tmp/herdr-tg-test-topics-{}.json",
            std::process::id()
        )));
        let probe = "test:pane";
        assert!(!st.is_unread(probe));
        assert!(st.mark_unread(probe));
        assert!(!st.mark_unread(probe));
        assert!(st.is_unread(probe));
        assert!(st.mark_read(probe));
        assert!(!st.mark_read(probe));
        assert!(!st.is_unread(probe));

        // persistence across reopen
        st.mark_unread("w9:p9");
        let re = TopicStorage::at(st.file_path.clone());
        assert!(re.is_unread("w9:p9"));
        let _ = std::fs::remove_file(&st.file_path);
    }
}
