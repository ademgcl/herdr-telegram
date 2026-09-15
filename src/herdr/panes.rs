use super::rpc::{rpc, rpc_t};
use crate::types::Res;
use serde_json::json;

pub async fn read_pane_output(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "pane.read",
        json!({"pane_id": pane, "source": "recent_unwrapped", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

/// Type text + submit in ONE call for blocked-agent dialogs. Two
/// round-trips (send_text, then send_keys enter) can split across a TUI
/// redraw: the text lands in the field while Enter hits a refocused
/// control, so the dialog proceeds WITHOUT the typed value.
pub async fn send_pane_input(socket: &str, pane: &str, text: &str) -> Res<()> {
    rpc_t(
        socket,
        "pane.send_input",
        json!({"pane_id": pane, "text": text, "keys": ["enter"]}),
        30,
    )
    .await?;
    Ok(())
}

/// Raw keys to a pane (no agent needed) — shell-mode `/keys`, quit flows.
pub async fn send_pane_keys(socket: &str, pane: &str, keys: &[&str]) -> Res<()> {
    rpc(
        socket,
        "pane.send_keys",
        json!({"pane_id": pane, "keys": keys}),
    )
    .await?;
    Ok(())
}

/// Shell output: line-based panes serve recent_unwrapped; fall back to
/// the visible viewport when they don't.
pub async fn read_shell_output(socket: &str, pane: &str, lines: u32) -> Res<String> {
    match read_pane_output(socket, pane, lines).await {
        Ok(out) => Ok(out),
        Err(_) => {
            let r = rpc(
                socket,
                "pane.read",
                json!({"pane_id": pane, "source": "visible", "lines": lines}),
            )
            .await?;
            Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
        }
    }
}

/// Every live pane id — includes agentless shells that `agent.list` hides.
/// Drives the shell-vs-dead decision in reconcile (keep shell topics, close
/// dead ones).
pub async fn list_panes(socket: &str) -> Res<Vec<String>> {
    let r = rpc(socket, "pane.list", json!({})).await?;
    let mut out = Vec::new();
    if let Some(arr) = r["panes"].as_array() {
        for p in arr {
            if let Some(id) = p["pane_id"].as_str() {
                out.push(id.to_string());
            }
        }
    }
    Ok(out)
}

/// Type text WITHOUT submitting — for TUI pickers/filters where Enter
/// means "confirm selection", not "send".
pub async fn type_pane_text(socket: &str, pane: &str, text: &str) -> Res<()> {
    rpc_t(
        socket,
        "pane.send_text",
        json!({"pane_id": pane, "text": text}),
        30,
    )
    .await?;
    Ok(())
}

/// Close a pane completely (agent or shell — herdr ends both).
pub async fn close_pane(socket: &str, pane: &str) -> Res<()> {
    rpc(socket, "pane.close", json!({"pane_id": pane})).await?;
    Ok(())
}

/// Fresh tab in `ws`, returning its root (shell) pane id.
pub async fn create_tab(socket: &str, ws: &str) -> Res<String> {
    let tab = rpc_t(socket, "tab.create", json!({"workspace_id": ws}), 30).await?;
    Ok(tab["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

/// Split a pane sideways in the same tab, returning the new pane id.
/// herdr names it with the workspace's next pane counter (`w8:p4`),
/// unlabeled, in the same tab.
pub async fn split_pane(socket: &str, pane: &str, direction: &str) -> Res<String> {
    let r = rpc_t(
        socket,
        "pane.split",
        json!({"target_pane_id": pane, "direction": direction}),
        30,
    )
    .await?;
    r["pane"]["pane_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "split returned no pane".into())
}
