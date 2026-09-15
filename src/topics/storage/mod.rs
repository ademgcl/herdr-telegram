use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::PathBuf,
    sync::Mutex,
};

#[cfg(test)]
mod tests;

#[derive(Serialize, Deserialize, Default)]
struct Store {
    #[serde(default)]
    topics: HashMap<String, i64>,
    #[serde(default)]
    unread: HashSet<String>,
    /// Stable short tags per pane ("o2") — survive restarts so topic
    /// names never reshuffle. Missing in old files → default empty.
    #[serde(default)]
    tags: HashMap<String, String>,
    /// Last 1:1 synced title per pane (herdr label or pane id). Compared
    /// before every rename so both directions converge without loops.
    #[serde(default)]
    titles: HashMap<String, String>,
    /// Pinned live-status message per pane (pane → message id).
    #[serde(default)]
    pins: HashMap<String, i64>,
    /// Topic icon custom-emoji ID set once per pane.
    #[serde(default)]
    icons: HashMap<String, String>,
}

pub struct TopicStorage {
    file_path: PathBuf,
    store: Mutex<Store>,
}

impl TopicStorage {
    pub fn new() -> Self {
        // Keep state self-contained next to the bot; migrate legacy XDG file once
        let path = crate::state::state_dir().join("topics.state");
        let home = env::var("HOME").unwrap_or_default();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/topics.json"));
        if !path.exists() && legacy.exists() && std::fs::copy(&legacy, &path).is_ok() {
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
        let Ok(txt) = fs::read_to_string(path) else {
            return Store::default();
        };
        // Legacy flat map {pane: thread} FIRST: without
        // deny_unknown_fields it would parse as an empty Store and wipe
        // every mapping. A current-format file always carries non-integer
        // values, so it can never match the flat shape.
        if let Ok(topics) = serde_json::from_str::<HashMap<String, i64>>(&txt) {
            return Store {
                topics,
                unread: HashSet::new(),
                tags: HashMap::new(),
                titles: HashMap::new(),
                pins: HashMap::new(),
                icons: HashMap::new(),
            };
        }
        serde_json::from_str::<Store>(&txt).unwrap_or_else(|_| {
            if !txt.trim().is_empty() {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let bak = PathBuf::from(format!("{}.corrupt-{}.bak", path.display(), secs));
                let _ = fs::copy(path, &bak);
            }
            Store::default()
        })
    }

    fn save(&self, s: &Store) {
        if let Some(parent) = self.file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(s) {
            let mut tmp = self.file_path.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            if fs::write(&tmp, json).is_ok()
                && let Err(e) = fs::rename(&tmp, &self.file_path)
            {
                eprintln!("[topics] rename {} failed: {e}", self.file_path.display());
            }
        }
    }

    pub fn get_thread(&self, pane: &str) -> Option<i64> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .topics
            .get(pane)
            .copied()
    }

    pub fn get_pane(&self, thread: i64) -> Option<String> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .topics
            .iter()
            .find(|(_, t)| **t == thread)
            .map(|(p, _)| p.clone())
    }

    pub fn insert(&self, pane: String, thread: i64) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        s.topics.insert(pane, thread);
        self.save(&s);
    }

    pub fn remove(&self, pane: &str) -> Option<i64> {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let prev = s.topics.remove(pane);
        s.unread.remove(pane);
        let untagged = s.tags.remove(pane);
        let untitled = s.titles.remove(pane);
        let unpinned = s.pins.remove(pane);
        let uniconed = s.icons.remove(pane);
        if prev.is_some()
            || untagged.is_some()
            || untitled.is_some()
            || unpinned.is_some()
            || uniconed.is_some()
        {
            self.save(&s);
        }
        prev
    }

    pub fn get_icon(&self, pane: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .icons
            .get(pane)
            .cloned()
    }

    pub fn set_icon(&self, pane: &str, icon: &str) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if s.icons.get(pane).map(|i| i.as_str()) != Some(icon) {
            s.icons.insert(pane.to_string(), icon.to_string());
            self.save(&s);
        }
    }

    pub fn get_tag(&self, pane: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tags
            .get(pane)
            .cloned()
    }

    pub fn set_tag(&self, pane: &str, tag: &str) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if s.tags.get(pane).map(|t| t.as_str()) != Some(tag) {
            s.tags.insert(pane.to_string(), tag.to_string());
            self.save(&s);
        }
    }

    pub fn get_title(&self, pane: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .titles
            .get(pane)
            .cloned()
    }

    pub fn set_title(&self, pane: &str, title: &str) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if s.titles.get(pane).map(|t| t.as_str()) == Some(title) {
            return;
        }
        s.titles.insert(pane.to_string(), title.to_string());
        self.save(&s);
    }

    /// Get-or-assign this pane's stable tag, atomically under one lock so
    /// concurrent topic creations never hand out the same tag twice.
    pub fn assign_tag(&self, pane: &str, kind: &str) -> String {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let pins = std::mem::take(&mut s.pins);
        if !pins.is_empty() {
            self.save(&s);
        }
        pins
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .topics
            .clone()
    }

    /// Clear all stored topics, tags, titles, unread, pins, and icons.
    pub fn clear_all(&self) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        s.topics.clear();
        s.unread.clear();
        s.tags.clear();
        s.titles.clear();
        s.pins.clear();
        s.icons.clear();
        self.save(&s);
    }
}
