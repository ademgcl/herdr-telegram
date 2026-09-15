//! Durable prompt intent: which pane owes a reply where. Recorded on
//! every submit, cleared on settle/cancel — boot re-arms watchers from
//! it so restarts stop eating replies.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingPrompt {
    pub chat: i64,
    pub thread: Option<i64>,
    pub prompt: String,
    pub started_unix: u64,
}

pub fn store_path() -> PathBuf {
    PathBuf::from("jobs.state")
}

pub fn load_file(path: &Path) -> HashMap<String, PendingPrompt> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_file(path: &Path, map: &HashMap<String, PendingPrompt>) {
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        PathBuf::from(format!("/tmp/herdr-tg-test-jobs-{}.json", std::process::id()))
    }

    #[test]
    fn test_roundtrip_and_corrupt() {
        let p = tmp();
        assert!(load_file(&p).is_empty());
        let mut m = HashMap::new();
        m.insert(
            "w8:p1".to_string(),
            PendingPrompt { chat: 1, thread: Some(6), prompt: "hi".into(), started_unix: 42 },
        );
        save_file(&p, &m);
        let back = load_file(&p);
        assert_eq!(back["w8:p1"].prompt, "hi");
        assert_eq!(back["w8:p1"].thread, Some(6));
        std::fs::write(&p, "not json{{").unwrap();
        assert!(load_file(&p).is_empty());
        let _ = std::fs::remove_file(&p);
    }
}
