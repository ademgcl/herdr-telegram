//! Routing memory: reply targets + focus. Split from `state`
//! (300-line file limit). Pure relocation — lock order and semantics
//! unchanged (torder → targets, memory-before-disk focus).
use super::State;
use crate::types::write_private;

/// Pure gate (tested): `last_msgs` serves forum-topic resets
/// (`copy_msg(forum, forum, mid)`) — only messages posted IN the forum
/// chat are valid entries. DM cards (same panes, different chat) must
/// never pollute it: their mids fail the copy AND evict real forum mids
/// under the cap-3 bound.
pub(crate) fn should_record_msg(forum: Option<i64>, chat: i64) -> bool {
    forum == Some(chat)
}

impl State {
    pub async fn remember(&self, chat: i64, msg_id: Option<i64>, pane: &str) {
        let Some(msg_id) = msg_id else { return };
        // Forum-only: `last_msgs` serves topic resets — in DM mode it is
        // write-only disk growth (no mapping ever prunes it), and DM mids
        // in forum mode poison the reset copy (single source: gate above).
        if should_record_msg(self.cfg.forum, chat) {
            self.topics.record_msg(pane, msg_id);
        }
        self.remember_reply(chat, msg_id, pane).await;
    }

    /// Reply-route memory for transient progress (targets only): the
    /// reset-copy `last_msgs` serves topic resets — a stale "thinking…"
    /// evicting real finals there resurrects corpses on remint.
    pub async fn remember_reply(&self, chat: i64, msg_id: i64, pane: &str) {
        // Lock order (never inverted anywhere): torder → targets.
        // LRU refresh on hit: hot cards survive the 512-cap, cold corpses evict first.
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        if map.insert((chat, msg_id), pane.to_string()).is_some() {
            ord.retain(|k| *k != (chat, msg_id));
            ord.push_back((chat, msg_id));
            return;
        }
        while map.len() >= 512 {
            match ord.pop_front() {
                Some(old) => {
                    map.remove(&old);
                }
                None => break,
            }
        }
        ord.push_back((chat, msg_id));
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
        // Fail-closed: only well-shaped pane ids persist — a garbage
        // focus routes bare DMs into the void until clear/restart.
        if !crate::types::valid_focus(pane) {
            eprintln!("[state] refusing invalid focus {pane:?}");
            return;
        }
        // Hold the focus lock across memory+disk (short local write, no
        // RPC): releasing after RAM let a slower writer's rename land
        // last with the loser's pane while RAM held the winner —
        // concurrent focuses must converge disk vs RAM on the same
        // winner. Memory first, disk second inside the critical section
        // (disk-first can resurrect a loser once the lock is held).
        let mut focus = self.focus.lock().await;
        *focus = Some(pane.to_string());
        let file = super::persist_paths::focus_file();
        // Unique tmp (never shared `<file>.tmp`): concurrent focuses
        // must not interleave into one torn file.
        let tmp = crate::types::unique_tmp(&file);
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

#[cfg(test)]
mod tests {
    use super::should_record_msg;

    #[test]
    fn test_record_gate_forum_chat_only() {
        // Forum chat records; DM chats never do (reset-copy poison).
        assert!(should_record_msg(Some(7), 7));
        assert!(!should_record_msg(Some(7), 9));
        // DM mode records nothing (write-only disk growth).
        assert!(!should_record_msg(None, 9));
    }

    #[tokio::test]
    async fn test_set_focus_disk_matches_ram_under_races() {
        // Lock-across-write: concurrent focuses serialize, so the last
        // RAM winner is also the last disk rename — disk ≠ RAM was the
        // pre-fix race (disk could hold a loser while RAM held winner).
        let (s, _dir) = crate::state::cancel::isolated_state();
        let mut handles = Vec::new();
        for i in 0..16 {
            let s = s.clone();
            handles.push(tokio::spawn(async move {
                s.set_focus(&format!("w1:p{i}")).await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let ram = s.get_focus().await.expect("focus set");
        let disk = std::fs::read_to_string(super::super::persist_paths::focus_file())
            .unwrap_or_default()
            .trim()
            .to_string();
        assert_eq!(disk, ram, "focus disk and RAM must converge");
    }
}
