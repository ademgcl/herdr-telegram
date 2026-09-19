//! Durable prompt intent: which pane owes a reply where. Recorded on
//! every submit, cleared on settle/cancel — boot re-arms watchers from
//! it so restarts stop eating replies.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingPrompt {
    pub chat: i64,
    pub thread: Option<i64>,
    pub prompt: String,
    pub started_unix: u64,
}

pub fn store_path() -> PathBuf {
    crate::state::state_dir().join("jobs.state")
}

pub fn load_file(path: &Path) -> HashMap<String, PendingPrompt> {
    let Ok(txt) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    if txt.trim().is_empty() {
        return HashMap::new();
    }
    serde_json::from_str(&txt).unwrap_or_else(|_| {
        // Corrupt intent file: back it up before dropping, mirroring
        // topics.storage — a truncated write must never silently eat
        // owed replies.
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bak = PathBuf::from(format!(
            "{}.corrupt-{}-{}.bak",
            path.display(),
            secs,
            std::process::id()
        ));
        let _ = std::fs::copy(path, &bak);
        crate::types::chmod_private(&bak);
        HashMap::new()
    })
}

pub fn save_file(path: &Path, map: &HashMap<String, PendingPrompt>) {
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if crate::types::write_private(&tmp, json.as_bytes()).is_ok() {
            if std::fs::rename(&tmp, path).is_err() {
                eprintln!("[jobs] intent rename failed (disk full?)");
            }
        } else {
            eprintln!("[jobs] intent write failed (disk full?)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        PathBuf::from(format!(
            "/tmp/herdr-tg-test-jobs-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn test_roundtrip_and_corrupt() {
        let p = tmp();
        assert!(load_file(&p).is_empty());
        let mut m = HashMap::new();
        m.insert(
            "w8:p1".to_string(),
            PendingPrompt {
                chat: 1,
                thread: Some(6),
                prompt: "hi".into(),
                started_unix: 42,
            },
        );
        save_file(&p, &m);
        let back = load_file(&p);
        assert_eq!(back["w8:p1"].prompt, "hi");
        assert_eq!(back["w8:p1"].thread, Some(6));
        std::fs::write(&p, "not json{{").unwrap();
        assert!(load_file(&p).is_empty());
        // Corrupt loads back the file up — clean up strays, not just `p`.
        let prefix = format!("{}.corrupt-", p.display());
        if let Ok(dir) = std::fs::read_dir("/tmp") {
            for e in dir.flatten() {
                let name = e.path().display().to_string();
                if name.starts_with(&prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let _ = std::fs::remove_file(&p);
    }
}
