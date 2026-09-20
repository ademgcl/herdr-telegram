//! Atomic disk layer for topic mappings: tmp+rename main, tmp+rename
//! `.prev` last-good, corrupt→`.bak` + prev fallback. Split from `mod`
//! (300-line file limit).
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct Store {
    #[serde(default)]
    pub topics: HashMap<String, i64>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
    #[serde(default)]
    pub titles: HashMap<String, String>,
    #[serde(default)]
    pub pins: HashMap<String, i64>,
    #[serde(default)]
    pub icons: HashMap<String, String>,
    #[serde(default)]
    pub last_msgs: HashMap<String, Vec<i64>>,
}

pub(crate) fn prev_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.prev", path.display()))
}

fn load_prev(path: &Path) -> Option<Store> {
    let prev = prev_path(path);
    if prev == *path {
        return None;
    }
    let ptxt = fs::read_to_string(&prev).ok()?;
    if let Ok(s) = serde_json::from_str::<Store>(&ptxt) {
        return Some(s);
    }
    serde_json::from_str::<HashMap<String, i64>>(&ptxt)
        .ok()
        .map(|topics| Store {
            topics,
            ..Default::default()
        })
}

/// Legacy flat main + newer full `.prev`: keep the flat topics but rescue
/// aux maps (tags/titles/pins/icons/last_msgs) for overlapping panes so a
/// stale flat main never amputates the backup on the next save.
fn merge_flat_with_prev(topics: HashMap<String, i64>, path: &Path) -> Store {
    let mut store = Store {
        topics,
        ..Default::default()
    };
    if let Some(prev) = load_prev(path) {
        let overlaps = store.topics.keys().any(|p| prev.topics.contains_key(p));
        let prev_has_aux = !prev.tags.is_empty()
            || !prev.titles.is_empty()
            || !prev.pins.is_empty()
            || !prev.icons.is_empty()
            || !prev.last_msgs.is_empty();
        if overlaps && prev_has_aux {
            for (p, v) in prev.tags {
                if store.topics.contains_key(&p) {
                    store.tags.insert(p, v);
                }
            }
            for (p, v) in prev.titles {
                if store.topics.contains_key(&p) {
                    store.titles.insert(p, v);
                }
            }
            for (p, v) in prev.pins {
                if store.topics.contains_key(&p) {
                    store.pins.insert(p, v);
                }
            }
            for (p, v) in prev.icons {
                if store.topics.contains_key(&p) {
                    store.icons.insert(p, v);
                }
            }
            for (p, v) in prev.last_msgs {
                if store.topics.contains_key(&p) {
                    store.last_msgs.insert(p, v);
                }
            }
        }
    }
    store
}

pub(crate) fn read_store(path: &Path) -> Store {
    let txt = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(s) = load_prev(path) {
                eprintln!("[topics] main missing, restored previous-good backup");
                return s;
            }
            return Store::default();
        }
    };
    // Empty main (truncated/touched file) is corruption, not a wipe:
    // consult `.prev` before returning empty (else every mapping
    // re-mints as a duplicate while last-good sits next to it).
    if txt.trim().is_empty() {
        if let Some(s) = load_prev(path) {
            eprintln!("[topics] main empty, restored previous-good backup");
            return s;
        }
        return Store::default();
    }
    // Full Store first: a legacy flat map parses as Store only when
    // empty (all-default), so trying Store first never eats a legacy
    // file — but trying flat first WOULD eat a stale flat main while a
    // newer full `.prev` backup sits next to it (tags/titles/pins/icons
    // wiped on the next save). Flat is the fallback, merged with prev aux.
    if let Ok(s) = serde_json::from_str::<Store>(&txt) {
        // Empty-Store-shaped legacy flat (`{}`): still check flat/prev
        // below when the Store carries nothing but the flat has topics.
        let store_empty = s.topics.is_empty()
            && s.tags.is_empty()
            && s.titles.is_empty()
            && s.pins.is_empty()
            && s.icons.is_empty()
            && s.last_msgs.is_empty();
        if !store_empty {
            return s;
        }
        if let Ok(topics) = serde_json::from_str::<HashMap<String, i64>>(&txt)
            && !topics.is_empty()
        {
            return merge_flat_with_prev(topics, path);
        }
        return s;
    }
    if let Ok(topics) = serde_json::from_str::<HashMap<String, i64>>(&txt) {
        return merge_flat_with_prev(topics, path);
    }
    // Corrupt (neither shape): prefer last-good, else quarantine + default.
    if let Some(s) = load_prev(path) {
        eprintln!("[topics] state corrupt, restored previous-good backup");
        return s;
    }
    if !txt.trim().is_empty() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bak = PathBuf::from(format!("{}.corrupt-{}.bak", path.display(), secs));
        let _ = fs::copy(path, &bak);
        crate::types::chmod_private(&bak);
    }
    Store::default()
}

pub(crate) fn write_store(path: &Path, s: &Store, backup: bool) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(s) else {
        eprintln!("[topics] state serialize failed — in-memory/disk diverging");
        return;
    };
    // Durable tmp+rename: fsync file before rename + dir after, so a
    // power loss can't leave a torn main (best effort, never panics).
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if crate::types::write_private(&tmp, json.as_bytes()).is_err() {
        eprintln!("[topics] state tmp write failed (disk full?) — in-memory/disk diverging");
        return;
    }
    {
        if let Ok(f) = fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        match fs::rename(&tmp, path) {
            Ok(()) => {
                if let Some(parent) = path.parent()
                    && let Ok(d) = fs::File::open(parent)
                {
                    let _ = d.sync_all();
                }
                if !backup {
                    return;
                }
                // Atomic backup, never a torn copy.
                let prev = prev_path(path);
                let mut ptmp = prev.as_os_str().to_owned();
                ptmp.push(".tmp");
                let ptmp = PathBuf::from(ptmp);
                let pretty = serde_json::to_string_pretty(s).unwrap_or_default();
                if crate::types::write_private(&ptmp, pretty.as_bytes()).is_ok() {
                    if let Ok(f) = fs::File::open(&ptmp) {
                        let _ = f.sync_all();
                    }
                    let _ = fs::rename(&ptmp, &prev);
                    if let Some(parent) = prev.parent()
                        && let Ok(d) = fs::File::open(parent)
                    {
                        let _ = d.sync_all();
                    }
                }
            }
            Err(e) => eprintln!(
                "[topics] rename {} failed: {e}",
                crate::types::collapse_home(&path.display().to_string(), &crate::types::home_dir())
            ),
        }
    }
}
