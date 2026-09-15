//! DM target resolution: which pane a command or bare text talks to.
//! Split from dm.rs under the 300-line file cap.
use crate::{state::AppState, types::AgentRow};

pub fn resolve_target(rows: &[AgentRow], spec: Option<&str>) -> Option<AgentRow> {
    match spec {
        None | Some("") => {
            if rows.len() == 1 { rows.first().cloned() } else { None }
        }
        Some(t) => rows.iter().find(|r| r.pane == t).cloned().or_else(|| {
            let m: Vec<_> = rows.iter().filter(|r| r.kind == t).collect();
            if m.len() == 1 { m.first().map(|r| (*r).clone()) } else { None }
        }),
    }
}

/// DM pane target: explicit pane/kind, else focus (which may be a rowless
/// shell pane — callers report that case themselves).
pub async fn dm_pane(s: &AppState, rows: &[AgentRow], arg: &str) -> Option<String> {
    match resolve_target(rows, Some(arg)) {
        Some(r) => Some(r.pane),
        None if arg.is_empty() => s.get_focus().await,
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
}
