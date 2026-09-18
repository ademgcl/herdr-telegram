//! DM target resolution: which pane a command or bare text talks to.
//! Split from dm.rs under the 300-line file cap.
use crate::{state::AppState, types::AgentRow};

pub fn resolve_target(rows: &[AgentRow], spec: Option<&str>) -> Option<AgentRow> {
    match spec {
        None | Some("") => {
            if rows.len() == 1 {
                rows.first().cloned()
            } else {
                None
            }
        }
        Some(t) => rows.iter().find(|r| r.pane == t).cloned().or_else(|| {
            // Exactly one kind match — never prompt the wrong agent.
            let mut it = rows.iter().filter(|r| r.kind == t);
            match (it.next(), it.next()) {
                (Some(r), None) => Some(r.clone()),
                _ => None,
            }
        }),
    }
}

/// Fail-closed reply: true when the reply names no live row (dead
/// pane). Callers refuse instead of falling through to focus/sole-agent
/// — acting on the wrong session from a corpse reply is a misroute.
/// (Rowless shell panes also mismatch: only bare text serves those, via
/// the shell fallback; every other command refuses visibly.)
pub fn unmatched_reply(rows: &[AgentRow], reply: &Option<String>) -> bool {
    reply
        .as_deref()
        .is_some_and(|p| rows.iter().all(|r| r.pane != p))
}

/// DM pane target for /quit + /kill: explicit pane/kind, else the
/// replied-to card's pane, else focus (which may be a rowless shell pane
/// — callers report that case themselves). An unmatched reply (dead
/// pane) refuses here via None ("who?") before the sole-agent shortcut
/// can shadow it into the wrong live session.
pub async fn dm_pane(
    s: &AppState,
    rows: &[AgentRow],
    arg: &str,
    reply: &Option<String>,
) -> Option<String> {
    // Corpse reply with exactly one live agent: the sole-agent shortcut
    // would otherwise act on the live session (idle /quit destroys work
    // with no confirm). Refuse before resolving.
    if arg.is_empty() && unmatched_reply(rows, reply) {
        return None;
    }
    match resolve_target(rows, Some(arg)) {
        Some(r) => Some(r.pane),
        None if arg.is_empty() => reply.clone().or(s.get_focus().await),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_target_by_pane() {
        let rows = vec![
            AgentRow {
                kind: "claude".into(),
                pane: "w1:p1".into(),
                title: "dev".into(),
                status: "idle".into(),
                ws: "w1".into(),
            },
            AgentRow {
                kind: "opencode".into(),
                pane: "w1:p2".into(),
                title: "fix".into(),
                status: "working".into(),
                ws: "w1".into(),
            },
        ];
        let found = resolve_target(&rows, Some("w1:p1"));
        assert_eq!(found.unwrap().pane, "w1:p1");

        let by_kind = resolve_target(&rows, Some("opencode"));
        assert_eq!(by_kind.unwrap().pane, "w1:p2");

        let nonexistent = resolve_target(&rows, Some("gemini"));
        assert!(nonexistent.is_none());
    }

    #[test]
    fn test_resolve_single_agent() {
        let rows = vec![AgentRow {
            kind: "claude".into(),
            pane: "w1:p1".into(),
            title: "".into(),
            status: "idle".into(),
            ws: "w1".into(),
        }];
        let found = resolve_target(&rows, None);
        assert_eq!(found.unwrap().pane, "w1:p1");
    }

    #[test]
    fn test_resolve_ambiguous_kind_fails_closed() {
        let rows = vec![
            AgentRow {
                kind: "opencode".into(),
                pane: "w1:p1".into(),
                title: "".into(),
                status: "idle".into(),
                ws: "w1".into(),
            },
            AgentRow {
                kind: "opencode".into(),
                pane: "w1:p2".into(),
                title: "".into(),
                status: "idle".into(),
                ws: "w1".into(),
            },
        ];
        assert!(resolve_target(&rows, Some("opencode")).is_none());
        assert!(resolve_target(&rows, Some("")).is_none());
    }

    #[test]
    fn test_unmatched_reply_flags_corpse_only() {
        let rows = vec![AgentRow {
            kind: "claude".into(),
            pane: "w1:p1".into(),
            title: "".into(),
            status: "idle".into(),
            ws: "w1".into(),
        }];
        // Dead pane: refuse. Live pane: serve. No reply: normal routing.
        // Rowless shell pane: mismatch (only bare text serves those).
        assert!(unmatched_reply(&rows, &Some("dead:p9".into())));
        assert!(!unmatched_reply(&rows, &Some("w1:p1".into())));
        assert!(!unmatched_reply(&rows, &None));
        assert!(unmatched_reply(&rows, &Some("w1:p7".into())));
    }

    #[tokio::test]
    async fn test_dm_pane_refuses_corpse_despite_sole_agent() {
        // Sole-agent shortcut must not shadow a corpse reply (bare
        // /quit would otherwise destroy the live session's work).
        let (s, _dir) = crate::state::cancel::isolated_state();
        let rows = vec![AgentRow {
            kind: "claude".into(),
            pane: "w1:p1".into(),
            title: "".into(),
            status: "idle".into(),
            ws: "w1".into(),
        }];
        assert_eq!(
            dm_pane(&s, &rows, "", &Some("dead:p9".into())).await,
            None
        );
        assert_eq!(
            dm_pane(&s, &rows, "", &None).await,
            Some("w1:p1".into())
        );
    }
}
