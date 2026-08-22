use std::{collections::HashMap, env, path::PathBuf, sync::Mutex};

pub struct TopicStorage {
    file_path: PathBuf,
    cache: Mutex<HashMap<String, i64>>,
}

impl TopicStorage {
    pub fn new() -> Self {
        let home = env::var("HOME").unwrap_or_default();
        let file_path = PathBuf::from(format!("{home}/.local/share/herdr-telegram/topics.json"));
        let initial_data = Self::read_from_disk(&file_path);
        Self {
            file_path,
            cache: Mutex::new(initial_data),
        }
    }

    fn read_from_disk(path: &PathBuf) -> HashMap<String, i64> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn get_thread(&self, pane: &str) -> Option<i64> {
        self.cache.lock().unwrap().get(pane).copied()
    }

    pub fn get_pane(&self, thread: i64) -> Option<String> {
        self.cache
            .lock()
            .unwrap()
            .iter()
            .find(|(_, t)| **t == thread)
            .map(|(p, _)| p.clone())
    }

    pub fn insert(&self, pane: String, thread: i64) {
        let mut map = self.cache.lock().unwrap();
        map.insert(pane, thread);
        self.save_to_disk(&map);
    }

    pub fn remove(&self, pane: &str) -> Option<i64> {
        let mut map = self.cache.lock().unwrap();
        let prev = map.remove(pane);
        if prev.is_some() {
            self.save_to_disk(&map);
        }
        prev
    }

    pub fn all_mappings(&self) -> HashMap<String, i64> {
        self.cache.lock().unwrap().clone()
    }

    fn save_to_disk(&self, map: &HashMap<String, i64>) {
        if let Some(parent) = self.file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(map) {
            let _ = std::fs::write(&self.file_path, json);
        }
    }
}
