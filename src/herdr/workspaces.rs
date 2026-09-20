use super::agents::get_agent;
use super::rpc::{is_not_found, rpc, rpc_t};
use crate::types::{AgentRow, Res, WorkspaceInfo};
use serde_json::json;

pub async fn list_workspaces(socket: &str) -> Res<Vec<WorkspaceInfo>> {
    let r = rpc(socket, "workspace.list", json!({})).await?;
    let mut out = Vec::new();
    if let Some(arr) = r["workspaces"].as_array() {
        for w in arr {
            out.push(WorkspaceInfo {
                id: w["workspace_id"].as_str().unwrap_or("?").into(),
                label: w["label"].as_str().unwrap_or("?").into(),
                number: w["number"].as_u64().unwrap_or(0),
            });
        }
    }
    Ok(out)
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

/// Find the `tg` space, creating it when missing. Returns
/// `(id, created)`: a just-created space ships a reusable root pane,
/// so callers must prefer it over `tab.create` (else p1 orphans).
pub async fn ensure_tg_space(socket: &str) -> Res<(String, bool)> {
    for w in list_workspaces(socket).await? {
        if w.label == "tg" {
            return Ok((w.id, false));
        }
    }
    let id = create_workspace(socket, "tg").await?;
    Ok((id, true))
}

/// Fresh tab in a known workspace id, returning its root pane.
/// Shared by spawns that must not reuse (existing spaces).
async fn create_tab_pane(socket: &str, ws: &str) -> Res<String> {
    let tab = rpc_t(socket, "tab.create", json!({"workspace_id": ws}), 30).await?;
    let pane = tab["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if pane.is_empty() {
        return Err("tab.create returned no pane".into());
    }
    Ok(pane)
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
        assert!(!verify_failure_reaps("herdr closed connection (empty reply)"));
        assert!(!verify_failure_reaps("no agent output yet"));
    }
}
