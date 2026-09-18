//! Routing memory: reply targets + focus. Split from `state`
//! (300-line file limit). Pure relocation — lock order and semantics
//! unchanged (torder → targets, memory-before-disk focus).
use super::State;
use crate::types::write_private;
use std::path::PathBuf;

impl State {
    pub async fn remember(&self, chat: i64, msg_id: Option<i64>, pane: &str) {
        let Some(msg_id) = msg_id else { return };
        self.topics.record_msg(pane, msg_id);
        // Lock order (never inverted anywhere): torder → targets.
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        while map.len() >= 512 {
            match ord.pop_front() {
                Some(old) => {
                    map.remove(&old);
                }
                None => break,
            }
        }
        if map.insert((chat, msg_id), pane.to_string()).is_none() {
            ord.push_back((chat, msg_id));
        }
    }

    /// Retire one card target (same lock order): `targets.remove` alone
    /// leaks the `torder` entry until the 512-cap overflow.
    pub async fn forget_target(&self, chat: i64, msg_id: i64) {
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        map.remove(&(chat, msg_id));
        ord.retain(|k| *k != (chat, msg_id));
    }

    pub async fn set_focus(&self, pane: &str) {
        // Memory first, disk second: concurrent focuses must converge
        // disk vs RAM on the same winner (disk-first can resurrect loser).
        *self.focus.lock().await = Some(pane.to_string());
        let file = Self::focus_file();
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if write_private(&tmp, pane.as_bytes()).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                eprintln!("[state] focus rename failed (disk full?)");
            }
        } else {
            eprintln!("[state] focus write failed (disk full?)");
        }
    }

    pub async fn get_focus(&self) -> Option<String> {
        self.focus.lock().await.clone()
    }
}
