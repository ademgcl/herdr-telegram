use std::{
    collections::HashMap,
    sync::Mutex,
    time::Instant,
};
use crate::{
    telegram::client::TelegramClient,
    topics::{names, storage::TopicStorage},
};

pub struct TopicManager {
    forum_id: Option<i64>,
    storage: TopicStorage,
    tg: TelegramClient,
    /// Last title set per pane (+when) — renames fire only on real change,
    /// never redundantly.
    last_title: Mutex<HashMap<String, (String, Instant)>>,
}

impl TopicManager {
    pub fn new(forum_id: Option<i64>, tg: TelegramClient) -> Self {
        Self {
            forum_id,
            storage: TopicStorage::new(),
            tg,
            last_title: Mutex::new(HashMap::new()),
        }
    }

    pub fn pane_of_thread(&self, thread: i64) -> Option<String> {
        self.storage.get_pane(thread)
    }

    /// This pane's stable tag, assigning on first sight (no-op after).
    pub fn tag(&self, pane: &str, kind: &str) -> String {
        self.storage.assign_tag(pane, kind)
    }

    pub fn all_mappings(&self) -> std::collections::HashMap<String, i64> {
        self.storage.all_mappings()
    }

    pub fn remove_mapping(&self, pane: &str) -> Option<i64> {
        self.last_title.lock().unwrap().remove(pane);
        self.storage.remove(pane)
    }

    /// Ensure the pane's topic exists (`{emoji} {tag} · {space}`, e.g.
    /// `🔄 o2 · herdr-telegram`) and return its thread. Creation only —
    /// later title tracking goes through `rename_to`, driven by genuine
    /// transitions (immediate) and debounce-confirmed settles.
    pub async fn ensure_topic(
        &self,
        pane: &str,
        kind: &str,
        space: &str,
        status: &str,
    ) -> Option<i64> {
        let forum = self.forum_id?;
        // Unknown kind (agent vanished mid-flight): never mint "?n" tags —
        // just route to the existing thread, if any.
        if kind == "?" {
            return self.storage.get_thread(pane);
        }
        let tag = self.storage.assign_tag(pane, kind);
        match self.storage.get_thread(pane) {
            Some(t) => Some(t),
            None => {
                let name = names::title(status, &tag, space);
                match self.tg.create_forum_topic(forum, &name).await {
                    Ok(thread) => {
                        println!("[topics] created topic #{thread} for {pane} ({name})");
                        self.storage.insert(pane.to_string(), thread);
                        self.last_title
                            .lock()
                            .unwrap()
                            .insert(pane.to_string(), (name, Instant::now()));
                        return Some(thread);
                    }
                    Err(e) => {
                        eprintln!("[topics] failed to create topic for {pane}: {e}");
                        None
                    }
                }
            }
        }
    }

    /// Rename the pane's topic unless it already shows this title.
    /// Callers decide timing: working/blocked rename immediately, settles
    /// only after debounce confirmation — that pacing (not a dumb timer)
    /// is what keeps titles fast yet flicker-free. Never notifies.
    pub async fn rename_to(&self, pane: &str, title: &str) {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return;
        };
        // Decide under the lock, act outside it — std guards can't cross await.
        let due = match self.last_title.lock().unwrap().get(pane) {
            // Fresh boot: set immediately (also migrates static titles).
            None => true,
            Some((prev, _)) => prev != title,
        };
        if !due {
            return;
        }
        let ok = match self.tg.rename_forum_topic(forum, thread, title).await {
            Ok(()) => true,
            // Server considers it equal (incl. its emoji-blind comparison)
            // — treat as synced so we don't retry-spam every observation.
            // NOTE: topic errors use underscores ("TOPIC_NOT_MODIFIED"),
            // unlike message edits ("message is not modified").
            Err(e) if e.to_string().contains("NOT_MODIFIED") => true,
            Err(e) => {
                eprintln!("[topics] rename #{thread} ({pane}) failed: {e}");
                false
            }
        };
        if ok {
            self.last_title
                .lock()
                .unwrap()
                .insert(pane.to_string(), (title.to_string(), Instant::now()));
        }
    }

    /// One-time cleanup of the retired pinned-status era: unpin leftovers.
    /// No-op once storage is clean.
    pub async fn retire_pins(&self) {
        let Some(forum) = self.forum_id else { return };
        for (pane, mid) in self.storage.take_pins() {
            println!("[topics] unpinning retired status pin #{mid} ({pane})");
            self.tg.unpin_msg(forum, mid).await;
        }
    }

    pub async fn close_topic(&self, pane: &str) {
        if let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) {
            let _ = self.tg.close_forum_topic(forum, thread).await;
        }
    }
}
