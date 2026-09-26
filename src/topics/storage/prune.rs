//! Orphan prune for topic storage (split from `mod`, 300-line limit).
use super::TopicStorage;

impl TopicStorage {
    /// One-time heal for orphans: `last_msgs` without a mapping
    /// is write-only (only topic resets read it). Called at boot in
    /// every mode — DM→forum switches orphan the same way.
    /// Threadless aux maps (tags/titles/pins/icons) leak the same way
    /// via failed creates — prune them here too (state grows by split:
    /// every map needs expiry + prune).
    pub fn prune_orphan_msgs(&self) {
        let mut s = self.lock();
        let orphans: Vec<String> = s
            .last_msgs
            .keys()
            .filter(|p| !s.topics.contains_key(*p))
            .cloned()
            .collect();
        let mut dirty = false;
        for p in orphans {
            s.last_msgs.remove(&p);
            dirty = true;
        }
        for key in ["tags", "titles", "pins", "icons"] {
            let dead: Vec<String> = match key {
                "tags" => s
                    .tags
                    .keys()
                    .filter(|p| !s.topics.contains_key(*p))
                    .cloned()
                    .collect(),
                "titles" => s
                    .titles
                    .keys()
                    .filter(|p| !s.topics.contains_key(*p))
                    .cloned()
                    .collect(),
                "pins" => s
                    .pins
                    .keys()
                    .filter(|p| !s.topics.contains_key(*p))
                    .cloned()
                    .collect(),
                _ => s
                    .icons
                    .keys()
                    .filter(|p| !s.topics.contains_key(*p))
                    .cloned()
                    .collect(),
            };
            for p in dead {
                match key {
                    "tags" => {
                        s.tags.remove(&p);
                    }
                    "titles" => {
                        s.titles.remove(&p);
                    }
                    "pins" => {
                        s.pins.remove(&p);
                    }
                    _ => {
                        s.icons.remove(&p);
                    }
                };
                dirty = true;
            }
        }
        if dirty {
            self.save(&s);
        }
    }
}
