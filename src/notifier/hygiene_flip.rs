//! DM-mode shell flips: split from `hygiene` (300-line file limit).
use super::hygiene::panes_once;
use crate::{
    handlers::shell_common::classify_shell_reuse, herdr::client::get_agent, state::AppState,
};
use std::collections::HashSet;

/// Leave-blocked cleanup verdict (pure, tested): a quit while blocked
/// must retire the content sig (else a same-content re-block after
/// re-enter stays silent on the stale match) — otherwise the dead card
/// locations resolve. Single source for the branch below.
#[derive(Debug, PartialEq)]
pub(crate) enum LeaveCleanup {
    Sig,
    Cards,
}

pub(crate) fn leave_cleanup(block_held: bool) -> LeaveCleanup {
    if block_held {
        LeaveCleanup::Sig
    } else {
        LeaveCleanup::Cards
    }
}

/// DM flip action (pure, tested): forum `classify_shell_reuse(false, …)`
/// parity — a vanished agent with a live watcher or owed intent must
/// retire (quit notice + quiet cancel), never status-flip only (the
/// watcher would spin forever and the owed reply would drop).
pub(crate) fn dm_flip_retires(owed: bool, job: bool) -> bool {
    !matches!(
        classify_shell_reuse(false, owed, job),
        crate::handlers::shell_common::ShellReuse::Ignore
    )
}

/// DM-mode shell flip: no topics exist, but `status` still drives the
/// limit scanner — a PC-side quit would keep its last agent status
/// forever and quota words in ordinary shell output would buzz false
/// ❗ cards. Flip shell-reused panes (status + episode only — no topic
/// card); dead panes stay for `reap_orphans`. Live watchers / owed
/// prompts retire via the shared vanish path (forum parity).
pub(crate) async fn flip_dm_shells(
    s: &AppState,
    live_panes: &HashSet<String>,
    pane_list: &mut Option<HashSet<String>>,
) {
    let missing: Vec<String> = {
        let st = s.status.lock().await;
        st.keys()
            .filter(|p| {
                !live_panes.contains(*p) && st.get(*p).map(|v| v != "shell").unwrap_or(false)
            })
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let Some(panes) = panes_once(s, pane_list).await else {
        return;
    };
    if panes.is_empty() {
        return;
    }
    for pane in missing {
        // Vanish confirm (reconcile forum parity): a single `list_agents`
        // dropout (Ok but partial) must not flip a live agent to shell
        // (limit scanner then skips it + wipes its stall episode).
        // Only a not-found answer confirms death; blips keep live.
        match get_agent(&s.cfg.socket, &pane).await {
            Ok(_) => continue,
            Err(e) => {
                if !crate::herdr::rpc::is_not_found(&e.to_string()) {
                    continue;
                }
            }
        }
        if panes.contains(&pane) {
            // Leave-blocked cleanup (reconcile forum parity): a quit
            // while blocked must retire its sig/cards, else a
            // same-content re-block after re-enter stays silent on the
            // stale sig and dead buttons stay tappable.
            match leave_cleanup(s.block_held(&pane).await) {
                LeaveCleanup::Sig => {
                    s.blocked_sig.lock().await.remove(&pane);
                }
                LeaveCleanup::Cards => {
                    crate::handlers::dialog::resolve_cards(s, &pane).await;
                }
            }
            // Forum classify parity: status-only flips leave a live
            // watcher spinning and drop the owed reply. was_shell is
            // false here (filter above); DM has no topic tags.
            let owed = s.pending.lock().await.get(&pane).cloned();
            let has_job = s.job_live(&pane).await;
            if dm_flip_retires(owed.is_some(), has_job) {
                crate::notifier::reconcile_vanished::retire_vanished(s, &pane, owed).await;
            } else {
                s.status
                    .lock()
                    .await
                    .insert(pane.clone(), "shell".to_string());
                s.clear_limit_episode(&pane).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leave_cleanup_blocked_retires_sig() {
        // Quit while blocked retires the sig (a same-content re-block
        // after re-enter must repost, never stay silent on stale sig);
        // an unblocked quit resolves the dead card locations instead.
        assert_eq!(leave_cleanup(true), LeaveCleanup::Sig);
        assert_eq!(leave_cleanup(false), LeaveCleanup::Cards);
    }

    #[test]
    fn test_dm_flip_retires_watcher_or_owed() {
        // Status-only flip (no job, no intent) stays a cheap mark-shell.
        assert!(!dm_flip_retires(false, false));
        // Live watcher or owed prompt must fully retire (forum parity) —
        // never leave the watcher spinning or drop the reply.
        assert!(dm_flip_retires(false, true));
        assert!(dm_flip_retires(true, false));
        assert!(dm_flip_retires(true, true));
    }
}
