//! Telegram `[new-space]` topic renames → herdr workspace renames.
//! Split from `titles` (300-line file limit): the bracket names the
//! space, the remainder names the pane — writing the whole `[new]
//! label` to the tab duplicated (`[old] [new] label`). This path
//! renames the workspace first, then the tab/pane only when the
//! remainder actually changed, and re-asserts `[new-space] core`
//! immediately so the topic stays 1:1 with space + pane names.
use crate::{
    handlers::title_rules::{space_label_taken, space_rename_core, stored_covers_label, title_core_for},
    handlers::titles_adopt::STATE_READ_ERR,
    herdr::{
        client::rename_workspace,
        labels::{PaneFacts, rename_pane, rename_tab, tab_labels},
    },
    state::AppState,
    topics::names::{self, norm_title},
    types::WorkspaceInfo,
};
use std::collections::HashMap;

/// Try the space-rename path: true = `[bracket]` named a different
/// space (handled — caller returns), false = same/no bracket (caller
/// takes the pane/tab path). Fail-closed throughout: duplicate labels
/// refuse, converged echoes skip, reset mid-flight aborts, RPC
/// failures warn, stale remints drop. Ambiguous replays (old bracket
/// arriving after a rename) read as last-writer-wins — same as tabs.
#[allow(clippy::too_many_arguments)] // Adopt already holds the fresh reads; a ctx struct only moves construction.
pub async fn try_adopt_space_rename(
    s: &AppState,
    chat: i64,
    th: i64,
    pane: &str,
    name: &str,
    facts: &HashMap<String, PaneFacts>,
    spaces: &[WorkspaceInfo],
    ws_id: &str,
    space: &str,
    kind: &str,
    multi: bool,
    tab_id: &str,
) -> bool {
    let Some((new_space, rest)) = names::space_rename_parts(name, space) else {
        return false;
    };
    if space_label_taken(spaces, ws_id, &new_space) {
        s.tg.send_msg(
            chat,
            Some(th),
            &format!("⚠️ space `{new_space}` already exists — rename ignored"),
            None,
        )
        .await;
        return true;
    }
    // Watchdog-parity core first (fresh tab read): labeled splits keep
    // the pane label, unlabeled splits keep `{tab} {tag}`, singles the
    // tab — never a bare tag that the next tick would rewrite.
    let wanted = space_rename_core(&rest, space, &new_space, kind);
    let tag = s.topics.tag_for(pane, kind);
    // Fail-closed: an unreadable tab list must not feed a defaulted map
    // into the write below (wrong title / wrong rename). Tab-less panes
    // need no read at all.
    let tabs = if tab_id.is_empty() {
        HashMap::new()
    } else {
        match tab_labels(&s.cfg.socket).await {
            Ok(t) => t,
            Err(_) => {
                s.tg.send_msg(chat, Some(th), STATE_READ_ERR, None).await;
                return true;
            }
        }
    };
    let pane_label = facts.get(pane).and_then(|f| f.label.as_deref());
    let tab_name = if tab_id.is_empty() {
        None
    } else {
        tabs.get(tab_id).map(|t| t.as_str())
    };
    let current = title_core_for(tab_name, &tag, multi, pane_label);
    // Converged-echo guard for stale space reads: our own sync landed
    // (stored already shows the new space + core) but `spaces` lags —
    // skip the redundant same-value rename (idempotent anyway).
    let stored = s.topics.topic_title(pane);
    let keep = wanted.clone().or(current.clone());
    if let Some(ref k) = keep
        && stored_covers_label(stored.as_deref(), &new_space, kind, k)
    {
        return true;
    }
    // Re-gate: reset may have started across the awaits above (the
    // entry gate in `titles` predates them) — never mutate mid-reset.
    if crate::handlers::reset::is_resetting() {
        return true;
    }
    if let Err(e) = rename_workspace(&s.cfg.socket, ws_id, &new_space).await {
        s.tg.send_msg(chat, Some(th), &format!("⚠️ space rename failed: {}", crate::types::mask_home(&e.to_string())), None).await;
        return true;
    }
    // Remainder decides the pane half: blank keeps the herdr core,
    // otherwise the remainder (shed against both spaces) renames the
    // tab (single) or pane label (split) when it actually changed.
    let mut final_core = current;
    if let Some(wc) = wanted
        && !wc.trim().is_empty()
        && norm_title(&wc) != final_core.as_deref().map(norm_title).unwrap_or_default()
    {
        let res = if !multi {
            if tab_id.is_empty() {
                Err("pane has no tab".to_string())
            } else {
                rename_tab(&s.cfg.socket, tab_id, &wc)
                    .await
                    .map_err(|e| crate::types::mask_home(&e.to_string()))
            }
        } else {
            rename_pane(&s.cfg.socket, pane, Some(wc.as_str()))
                .await
                .map_err(|e| crate::types::mask_home(&e.to_string()))
        };
        match res {
            Ok(()) => {
                final_core = Some(wc);
            }
            Err(reason) => {
                s.tg.send_msg(chat, Some(th), &format!("⚠️ space renamed, pane rename failed — retry: {reason}"), None).await;
            }
        }
    }
    if crate::handlers::reset::is_resetting() {
        return true;
    }
    if !s.topics.note_title_if_thread(pane, th, name) {
        println!("[titles] stale space rename dropped for {pane}");
        return true;
    }
    s.topics.note_kind(pane, kind);
    let core = final_core.filter(|c| !c.trim().is_empty()).unwrap_or(tag);
    if let Some(p) = s.topics.sync_title(pane, &names::format_title(&new_space, &core, kind)).await {
        crate::handlers::dialog::retire_dialog(s, &p).await;
    }
    println!("[titles] rename #{th} → space {ws_id} ({pane}) {new_space:?}");
    true
}
