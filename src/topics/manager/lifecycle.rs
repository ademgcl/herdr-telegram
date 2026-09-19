//! Topic lifecycle: reopen/close/delete, shell badging, card
//! retirement, and mapping removal.
//! Split from `manager` (300-line file limit).
use super::TopicManager;
use crate::topics::names;

/// Naming inputs for a reset mint, split after the tab-name refactor:
/// the identity card keeps the pane/agent title while the topic itself is
/// named from the tab core (watchdog parity via `naming_core` — one
/// source, so reset can never reformat a verbatim the watchdog keeps).
pub struct ResetNames<'a> {
    /// Identity card subtitle (pane label ‖ agent title).
    pub card: Option<&'a str>,
    /// Tab-derived core; None → stable tag default (never verbatim,
    /// matching the watchdog which never preserves a bare tag).
    pub core: Option<&'a str>,
    /// Split-tab flag: single-pane verbatim needs the tab core; split
    /// panes additionally keep a stored title equal to the pane label
    /// (`card` carries it when set — see reset_desired_title).
    pub multi: bool,
}

impl TopicManager {
    /// F1: Reopen a closed forum topic (e.g. when agent transitions to working).
    /// Returns the pane when the mapping was pruned (caller retires its
    /// dialog generation); None otherwise (ok, noop, or failed-kept).
    pub async fn reopen_topic(&self, pane: &str) -> Option<String> {
        let (Some(forum), Some(thread)) = (self.forum_id, self.storage.get_thread(pane)) else {
            return None;
        };
        match self.tg.reopen_forum_topic(forum, thread).await {
            Ok(()) => None,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    // Human-deleted topic: drop the corpse so the next
                    // ensure recreates instead of reusing a dead thread.
                    // Compare-and-delete: never kill a fresh remint.
                    if self.remove_mapping_if_thread(pane, thread) {
                        println!("[topics] pruned deleted topic #{thread} ({pane}) on reopen");
                        return Some(pane.to_string());
                    }
                    return None;
                }
                eprintln!("[topics] reopen topic #{thread} ({pane}) failed: {}", self.tg.redact(&e.to_string()));
                None
            }
        }
    }

    /// Badge a live-but-agentless pane as shell: no title touch (titles
    /// sync 1:1 with herdr names), just ensures the topic. Returns true
    /// when the mapping was pruned this call (caller retires its dialog
    /// generation — uniform with every other sync site).
    pub async fn mark_shell(&self, pane: &str) -> bool {
        self.sync_topic_prune(pane, "shell", "?").await.1
    }

    /// Race-free close: RPCs the SNAPSHOT thread id directly, never
    /// re-reading the mapping (a remint landing between check and RPC
    /// must not have its fresh topic closed). Compare-deletes the mapping
    /// only when it still equals the snapshot.
    pub async fn close_topic_for_thread(&self, pane: &str, thread: i64) -> bool {
        let Some(forum) = self.forum_id else {
            return true;
        };
        let ok = match self.tg.close_forum_topic(forum, thread).await {
            Ok(()) => true,
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping_if_thread(pane, thread);
                    return true;
                }
                eprintln!("[topics] close topic #{thread} ({pane}) failed: {}", self.tg.redact(&e.to_string()));
                false
            }
        };
        if ok {
            self.remove_mapping_if_thread(pane, thread);
        }
        ok
    }

    /// Race-free delete: RPCs the SNAPSHOT thread id directly, never
    /// re-reading the mapping (parity with `close_topic_for_thread` — a
    /// remint landing between snapshot and RPC must not have its fresh
    /// topic deleted). Compare-deletes only when still current.
    pub async fn delete_topic_for_thread(&self, pane: &str, thread: i64) -> bool {
        let Some(forum) = self.forum_id else {
            return true;
        };
        match self.tg.delete_forum_topic(forum, thread).await {
            Ok(()) => {
                self.remove_mapping_if_thread(pane, thread);
                println!("[topics] deleted topic #{thread} ({pane})");
                true
            }
            Err(e) => {
                if crate::telegram::topic_missing(&e.to_string()) {
                    self.remove_mapping_if_thread(pane, thread);
                    return true;
                }
                eprintln!("[topics] delete topic #{thread} ({pane}) failed: {}", self.tg.redact(&e.to_string()));
                false
            }
        }
    }

    /// Durable shell marker: creation tags (`sh<n>`) and the kind icon
    /// survive restarts, unlike `status` (empty at boot).
    /// Guards the already-shell retire decision when status is unknown —
    /// without it the first tick after a restart misclassifies a live
    /// shell long-run as a fresh agent→shell flip (ghost quit card +
    /// eaten shell intent). Strict `sh<n>` shape plus icon cross-check:
    /// in steady state an unknown future agent kind starting with "sh"
    /// keeps its agent icon and cannot collide. Residuals: a crash in
    /// the assign-tag→set-icon window leaves icon None + sh tag
    /// (transient false positive until the next icon sync); pre-guard
    /// "?"-stamped rows are not healed (the guard only stops new ones).
    pub fn is_shell_tagged(&self, pane: &str) -> bool {
        if self.storage.get_icon(pane).as_deref() == Some(names::context_icon_emoji_id("shell")) {
            return true;
        }
        // Durable sh<n> tag regardless of icon: a user-customized shell
        // icon must not lose the marker (else restart misreads live
        // shell as a fresh flip — ghost quit + eaten intent). Future
        // unknown kinds starting with "sh" keep bot icons (fallback 🤖),
        // so only None (legacy) or non-bot (custom) icons count — a bot
        // fallback with an sh tag is an agent, not a shell.
        let tag_is_sh = self.storage.get_tag(pane).is_some_and(|t| {
            t.starts_with("sh")
                && !t[2..].is_empty()
                && t[2..].chars().all(|c| c.is_ascii_digit())
        });
        if !tag_is_sh {
            return false;
        }
        match self.storage.get_icon(pane) {
            None => true,
            Some(id) => !names::is_bot_icon(&id),
        }
    }

    /// F6: Record recent message ID for this pane.
    pub fn record_msg(&self, pane: &str, mid: i64) {
        self.storage.record_msg(pane, mid);
    }

    /// F6: Get recent message IDs for this pane.
    pub fn get_recent_msgs(&self, pane: &str) -> Vec<i64> {
        self.storage.get_recent_msgs(pane)
    }

    /// F6 + F2: Paced reset for a single pane:
    /// 1. Look up existing thread and recent messages before deletion.
    /// 2. Mint new topic on Telegram.
    /// 3. Record the new mapping immediately (insert-before-delete: a
    ///    crash must never leave the mapping pointing at a deleted
    ///    thread — a leftover old topic is just an orphan, while the
    ///    mapping stays valid).
    /// 4. Set kind icon (or preserve user icon).
    /// 5. Copy recent messages from the old topic to the new topic (F6).
    /// 6. Post fresh identity card (F2, never pinned).
    /// 7. Delete old topic on Telegram (if one existed).
    pub async fn reset_topic(
        &self,
        pane: &str,
        kind: &str,
        space: &str,
        status: &str,
        names: ResetNames<'_>,
        branch: Option<&str>,
    ) -> Option<i64> {
        let forum = self.forum_id?;
        let old_thread = self.storage.get_thread(pane);
        let tag = self.storage.assign_tag(pane, kind);
        // 1:1 Format-B always: user text survives reset re-wrapped
        // bare with the space. Split tabs keep pane-label renames
        // pane-label renames (`names.core` already carries the label
        // when set — watchdog parity, no flap). Same `naming_core`
        // source as the watchdog: no drift.
        let pre = self.storage.get_title(pane);
        let name = match names.core {
            Some(core) => crate::handlers::title_rules::reset_desired_title(
                pre.as_deref(),
                space,
                core,
                kind,
                names.multi,
                names.card,
            ),
            None => names::format_title(space, &tag, kind),
        };

        // Mint new topic
        let color = names::workspace_icon_color(space);
        let new_thread = match self.tg.create_forum_topic(forum, &name, Some(color)).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[reset] create topic failed for {pane}: {}", self.tg.redact(&e.to_string()));
                return None;
            }
        };

        // Record the mapping before touching the old topic (see step 3).
        self.storage
            .insert_with_title(pane.to_string(), new_thread, &name);

        // Icon: preserve user-customized icon if one was set, else kind
        // icon — stored only on success so a failed write retries next
        // tick instead of freezing a stale glyph.
        let icon = self
            .storage
            .get_icon(pane)
            .unwrap_or_else(|| names::context_icon_emoji_id(kind).to_string());
        if self.tg.set_topic_icon(forum, new_thread, &icon).await.is_ok() {
            self.storage.set_icon(pane, &icon);
        }

        // F6: copy recent messages to the new topic before old topic is deleted
        let mids = self.storage.get_recent_msgs(pane);
        for mid in mids {
            let _ = self.tg.copy_msg(forum, forum, mid, Some(new_thread)).await;
        }

        // F2: fresh identity card, tracked by id for status edits —
        // never pinned.
        let card = crate::ui::build_identity_card_text(kind, pane, space, status, names.card, branch);
        if let Some(mid) = self.tg.send_msg(forum, Some(new_thread), &card, None).await
            && !self.storage.set_pin_if_thread(pane, new_thread, mid)
        {
            println!("[reset] pin reminted during migrate for {pane} — dropping stale mid");
        }

        // Delete old topic now that the mapping already points at the
        // new one (a crash here leaves an orphan, never a corpse mapping).
        // Loud on failure: nothing enumerates Telegram topics, so a
        // failed delete orphans silently without this line.
        if let Some(old) = old_thread
            && let Err(e) = self.tg.delete_forum_topic(forum, old).await
        {
            eprintln!("[reset] delete old topic #{old} failed (orphaned): {}", self.tg.redact(&e.to_string()));
        }

        println!(
            "[reset] migrated topic for {pane}: #{:?} → #{new_thread}",
            old_thread
        );
        Some(new_thread)
    }
}
