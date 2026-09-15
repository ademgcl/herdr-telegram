use super::agents::get_agent;
use super::rpc::{rpc, rpc_t};
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
    Ok(r["workspace"]["workspace_id"].as_str().unwrap_or("").into())
}

pub async fn ensure_tg_space(socket: &str) -> Res<String> {
    for w in list_workspaces(socket).await? {
        if w.label == "tg" {
            return Ok(w.id);
        }
    }
    create_workspace(socket, "tg").await
}

pub async fn spawn_agent(socket: &str, kind: &str, target_ws: Option<&str>) -> Res<AgentRow> {
    let ws = match target_ws {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => ensure_tg_space(socket).await?,
    };
    let tab = rpc_t(socket, "tab.create", json!({"workspace_id": ws}), 30).await?;
    let pane = tab["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if pane.is_empty() {
        return Err("tab.create returned no pane".into());
    }

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
        // Don't leak the tab when the start fails (bad kind, etc.).
        let _ = super::panes::close_pane(socket, &pane).await;
        return Err(e);
    }

    let detail = get_agent(socket, &pane).await?;
    Ok(detail.into())
}
