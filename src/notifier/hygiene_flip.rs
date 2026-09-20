//! DM-mode shell flips: split from `hygiene` (300-line file limit).
use super::hygiene::panes_once;
use crate::{herdr::client::get_agent, state::AppState};
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

/// DM-mode shell flip: no topics exist, but `status` still drives the
/// limit scanner — a PC-side quit would keep its last agent status
/// forever and quota words in ordinary shell output would buzz false
/// ❗ cards. Flip shell-reused panes (status + episode only — no topic,
/// no report); dead panes stay for `reap_orphans`.
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
            s.status
                .lock()
                .await
                .insert(pane.clone(), "shell".to_string());
            s.clear_limit_episode(&pane).await;
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
}
