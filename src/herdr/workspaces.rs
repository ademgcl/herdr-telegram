use super::agents::get_agent;
use super::rpc::{is_not_found, rpc, rpc_t};
use crate::types::{AgentRow, Res, WorkspaceInfo};
use serde_json::json;

/// Fail-closed parse of `workspace.list`: a missing/null `workspaces`
/// key is an error, never `Ok(vec![])` (empty-success made every caller
/// treat a degraded reply as "no spaces" and mint duplicates). Pure so
/// the shape is unit-tested without the socket.
pub(crate) fn parse_workspaces(r: &serde_json::Value) -> Res<Vec<WorkspaceInfo>> {
    let arr = r["workspaces"]
        .as_array()
        .ok_or("workspace.list missing workspaces")?;
    let mut out = Vec::with_capacity(arr.len());
    for w in arr {
        out.push(WorkspaceInfo {
            id: w["workspace_id"].as_str().unwrap_or("?").into(),
            label: w["label"].as_str().unwrap_or("?").into(),
            number: w["number"].as_u64().unwrap_or(0),
        });
    }
    Ok(out)
}

pub async fn list_workspaces(socket: &str) -> Res<Vec<WorkspaceInfo>> {
    let r = rpc(socket, "workspace.list", json!({})).await?;
    parse_workspaces(&r)
}

pub async fn create_workspace(socket: &str, label: &str) -> Res<String> {
    let r = rpc_t(socket, "workspace.create", json!({"label": label}), 30).await?;
    // Fail-closed at the source (create_tab parity): a missing id must
    // never flow downstream as a ghost Ok("") — every caller guards
    // empty today only because this once returned it.
    let id = r["workspace"]["workspace_id"].as_str().unwrap_or("");
    if id.is_empty() {
        return Err("space create returned no id".into());
    }
    Ok(id.to_string())
}

/// Rename a workspace's display label (Telegram `[space]`-only renames
/// map here — the bracket names the space, not the pane). Param shape
/// verified live against herdr (`workspace_not_found` on bogus id proves
/// the shape; `id`-keyed variants reject as `invalid_request`).
pub async fn rename_workspace(socket: &str, ws_id: &str, label: &str) -> Res<()> {
    rpc(
        socket,
        "workspace.rename",
        json!({"workspace_id": ws_id, "label": label}),
    )
    .await?;
    Ok(())
}

/// Pick the `tg` space id from a listing, if present. Pure so the
/// race-adopt rule in `ensure_tg_space` is unit-tested (single source,
/// never a duplicated literal).
fn tg_space_id(spaces: &[WorkspaceInfo]) -> Option<String> {
    spaces
        .iter()
        .find(|w| w.label == "tg")
        .map(|w| w.id.clone())
}

/// Find the `tg` space, creating it when missing. Returns
/// `(id, created)`: a just-created space ships a reusable root pane,
/// so callers must prefer it over `tab.create` (else p1 orphans).
pub async fn ensure_tg_space(socket: &str) -> Res<(String, bool)> {
    if let Some(id) = tg_space_id(&list_workspaces(socket).await?) {
        return Ok((id, false));
    }
    match create_workspace(socket, "tg").await {
        Ok(id) => Ok((id, true)),
        Err(e) => {
            // List-then-create race: a concurrent caller minted `tg`
            // between our list and create (ours then fails or would
            // duplicate) — adopt the winner instead of erroring. Only a
            // still-missing `tg` propagates the original failure.
            if let Ok(spaces) = list_workspaces(socket).await
                && let Some(id) = tg_space_id(&spaces)
            {
                return Ok((id, false));
            }
            Err(e)
        }
    }
}

/// Fresh tab in a known workspace id, returning its root pane.
/// Single-sourced on `panes::create_tab` (same RPC, same fail-closed
/// empty-pane guard) — a duplicated literal here re-drifts.
async fn create_tab_pane(socket: &str, ws: &str) -> Res<String> {
    super::panes::create_tab(socket, ws).await
}

pub async fn spawn_agent(socket: &str, kind: &str, target_ws: Option<&str>) -> Res<AgentRow> {
    let (ws, fresh) = match target_ws {
        Some(w) if !w.is_empty() => (w.to_string(), false),
        _ => ensure_tg_space(socket).await?,
    };
    // Fresh space ships a root pane: start the agent there instead of
    // orphaning p1 with a new tab. Existing spaces always get a tab
    // (probing them would waste RPCs and risk a live pane).
    // `ours` tracks tabs WE minted: only those are closed when the
    // start fails — a reused root stays as a usable shell.
    let (pane, ours) = if fresh {
        match super::panes::await_fresh_root(socket, &ws).await {
            Some(p) => (p, false),
            None => (create_tab_pane(socket, &ws).await?, true),
        }
    } else {
        (create_tab_pane(socket, &ws).await?, true)
    };

    // Millis + pid: same-second double-spawns (double-tapped N) must not
    // collide on the agent name.
    let name = format!(
        "tg-{kind}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    if let Err(e) = rpc_t(
        socket,
        "agent.start",
        json!({"name": name, "kind": kind, "pane_id": pane}),
        90,
    )
    .await
    {
        // Don't leak a minted tab when the start fails (bad kind,
        // etc.). A reused root is NOT ours: leave it as a shell.
        if ours {
            let _ = super::panes::close_pane(socket, &pane).await;
        }
        return Err(e);
    }

    // The verify read can fail on a herdr hiccup AFTER the start
    // succeeded: don't leak the minted tab+agent (retry would mint
    // another). A reused root is NOT ours: leave it as a shell.
    // Fail-closed: only a confirmed-dead pane reaps the minted tab —
    // a transient read failure keeps it (never kill a healthy
    // just-started agent; retry would mint a second tab+agent).
    let detail = match get_agent(socket, &pane).await {
        Ok(d) => d,
        Err(e) => {
            if ours && verify_failure_reaps(&e.to_string()) {
                let _ = super::panes::close_pane(socket, &pane).await;
            }
            return Err(e);
        }
    };
    Ok(detail.into())
}

/// Verify-read verdict (pure, tested): only a confirmed-dead pane
/// reaps the minted tab (agent died instantly). Single source for the
/// spawn arm above.
fn verify_failure_reaps(msg: &str) -> bool {
    is_not_found(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_failure_reaps_dead_only() {
        // Instant-death reaps the minted tab (no leak, no double-mint).
        assert!(verify_failure_reaps("agent_not_found"));
        assert!(verify_failure_reaps("no such pane"));
        // Transient blips keep the pane (fail-closed): the agent is
        // plausibly healthy, and retry must adopt it, not kill it.
        assert!(!verify_failure_reaps("herdr agent.start timed out"));
        assert!(!verify_failure_reaps(
            "herdr closed connection (empty reply)"
        ));
        assert!(!verify_failure_reaps("no agent output yet"));
    }

    #[test]
    fn test_parse_workspaces_fail_closed_on_missing_key() {
        use serde_json::json;
        // Missing/null key is an error — never empty-success (callers
        // would mint duplicate `tg` spaces on a degraded reply).
        assert!(parse_workspaces(&json!({})).is_err());
        assert!(parse_workspaces(&json!({"workspaces": null})).is_err());
        assert!(parse_workspaces(&json!({"other": []})).is_err());
        // Empty array is a real answer (Ok, not Err).
        assert_eq!(
            parse_workspaces(&json!({"workspaces": []})).unwrap(),
            vec![]
        );
        let one = parse_workspaces(&json!({
            "workspaces": [{"workspace_id": "w1", "label": "tg", "number": 3}]
        }))
        .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].id, "w1");
        assert_eq!(one[0].label, "tg");
        assert_eq!(one[0].number, 3);
    }

    #[test]
    fn test_tg_space_id_picks_tg_only() {
        // The list-then-create race-adopt in `ensure_tg_space` keys on
        // this: a concurrent mint must be adopted, never duplicated.
        fn ws(id: &str, label: &str) -> WorkspaceInfo {
            WorkspaceInfo {
                id: id.into(),
                label: label.into(),
                number: 0,
            }
        }
        assert_eq!(
            tg_space_id(&[ws("w1", "dev"), ws("w2", "tg")]),
            Some("w2".to_string())
        );
        // No `tg` anywhere: caller mints (create path), never adopts.
        assert_eq!(tg_space_id(&[ws("w1", "dev")]), None);
        assert_eq!(tg_space_id(&[]), None);
        // Label match is exact: near-misses never adopt the wrong space.
        assert_eq!(tg_space_id(&[ws("w3", "tg2")]), None);
    }
}
