//! Tag identity (pane→`sh<n>`/`o<n>`): stable short ids (split from
//! `storage/mod`, 300-line file limit). Reassigns on kind-family flips
//! so `is_shell_tagged` never reads a stale family tag.
use super::TopicStorage;
use crate::topics::names;

impl TopicStorage {
    /// Get-or-assign stable tag atomically: concurrent creates never
    /// hand out the same tag twice. Reassigns on kind-family flips
    /// (shell `sh*` vs agent): a pane reused across a shell↔agent flip
    /// must not keep the old family's tag, or `is_shell_tagged` reads
    /// the stale tag as shell (ghost quit + eaten intent).
    pub fn assign_tag(&self, pane: &str, kind: &str) -> String {
        let mut s = self.lock();
        if let Some(t) = s.tags.get(pane).cloned() {
            let was_sh = t.starts_with("sh")
                && !t[2..].is_empty()
                && t[2..].chars().all(|c| c.is_ascii_digit());
            let want_sh = kind.trim().eq_ignore_ascii_case("shell");
            if was_sh == want_sh {
                return t;
            }
            let taken: Vec<String> = s
                .tags
                .iter()
                .filter(|(p, _)| *p != pane)
                .map(|(_, v)| v.clone())
                .collect();
            let tag = names::assign(&taken, kind);
            s.tags.insert(pane.to_string(), tag.clone());
            self.save(&s);
            return tag;
        }
        let taken: Vec<String> = s.tags.values().cloned().collect();
        let tag = names::assign(&taken, kind);
        s.tags.insert(pane.to_string(), tag.clone());
        self.save(&s);
        tag
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
}
