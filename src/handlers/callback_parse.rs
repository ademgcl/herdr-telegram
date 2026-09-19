//! Callback routing cuts + stale-pane guards. Split from `callback`
//! (300-line file limit).
use crate::{
    herdr::client::{get_agent, list_panes},
    state::AppState,
};

/// True when the pane still exists (agent or shell). Stale card taps
/// must never arm waiters, move focus, or remember targets for dead
/// panes — the next message would route into the void.
pub(crate) async fn pane_live(s: &AppState, pane: &str) -> bool {
    if get_agent(&s.cfg.socket, pane).await.is_ok() {
        return true;
    }
    // Fail-open: a failed list call must not read as "dead" (every stale
    // tap would false-gone during a herdr blip); the tap itself then
    // fails gracefully with a visible error.
    list_panes(&s.cfg.socket)
        .await
        .map(|l| l.contains(&pane.to_string()))
        .unwrap_or(true)
}

/// Dead pane tapped: retire the stale card, route nothing.
pub(crate) async fn gone_card(s: &AppState, chat: i64, msg_id: i64, _pane: &str) {
    s.tg.edit_msg(chat, msg_id, crate::ui::UNKNOWN_TARGET, None).await;
    s.forget_target(chat, msg_id).await;
}

/// Dead-pane guard for `B:`/`M:`/`X:` taps: parse the pane out of the
/// rest and retire the stale card when it is gone. Returns false when
/// the caller must stop.
pub(crate) async fn live_target(s: &AppState, chat: i64, msg_id: i64, r: &str) -> bool {
    match split_action(r) {
        Some((_, pane)) if !pane_live(s, pane).await => {
            gone_card(s, chat, msg_id, pane).await;
            false
        }
        _ => true,
    }
}

/// First routing cut: `B:opt2:wG:p1` → `("B", Some("opt2:wG:p1"))`.
/// Pure so the colon rules are unit-tested, not just eyeballed.
pub(crate) fn split_head(data: &str) -> (&str, Option<&str>) {
    match data.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (data, None),
    }
}

/// Second cut for pane-carrying actions: `opt2:wG:p1` →
/// `Some(("opt2", "wG:p1"))`. Pane ids contain ':' so only the FIRST
/// colon splits — multi-split thinking must never creep in.
pub(crate) fn split_action(rest: &str) -> Option<(&str, &str)> {
    rest.split_once(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_head() {
        assert_eq!(split_head("n"), ("n", None));
        assert_eq!(split_head("B:opt2:wG:p1"), ("B", Some("opt2:wG:p1")));
        assert_eq!(split_head("X:kill:w1:p2"), ("X", Some("kill:w1:p2")));
    }

    #[test]
    fn test_split_action_keeps_pane_whole() {
        // Pane ids contain ':' — only the first colon splits.
        assert_eq!(split_action("opt2:wG:p1"), Some(("opt2", "wG:p1")));
        assert_eq!(split_action("kill:w1:p2"), Some(("kill", "w1:p2")));
        assert_eq!(split_action("allow"), None);
    }
}
