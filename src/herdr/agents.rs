use super::rpc::rpc;
use crate::types::{AgentDetail, AgentRow, Res};
use serde_json::json;

pub async fn list_agents(socket: &str) -> Res<Vec<AgentRow>> {
    let r = rpc(socket, "agent.list", json!({})).await?;
    let mut out = Vec::new();
    if let Some(arr) = r["agents"].as_array() {
        for a in arr {
            out.push(AgentRow {
                kind: a["agent"].as_str().unwrap_or("?").into(),
                pane: a["pane_id"].as_str().unwrap_or("?").into(),
                title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
                status: a["agent_status"].as_str().unwrap_or("unknown").into(),
                ws: a["workspace_id"].as_str().unwrap_or("?").into(),
            });
        }
    }
    Ok(out)
}

pub async fn get_agent(socket: &str, pane: &str) -> Res<AgentDetail> {
    let v = rpc(socket, "agent.get", json!({"target": pane})).await?;
    let a = &v["agent"];
    Ok(AgentDetail {
        kind: a["agent"].as_str().unwrap_or("?").into(),
        pane: a["pane_id"].as_str().unwrap_or(pane).into(),
        title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
        status: a["agent_status"].as_str().unwrap_or("unknown").into(),
        ws: a["workspace_id"].as_str().unwrap_or("?").into(),
        cwd: a["foreground_cwd"]
            .as_str()
            .or(a["cwd"].as_str())
            .unwrap_or("")
            .into(),
    })
}

pub async fn read_agent_output(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "agent.read",
        json!({"target": pane, "source": "recent_unwrapped", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

/// Visible-viewport read: the ONLY source herdr serves while a pane is
/// blocked/working on an alternate screen (recent_unwrapped errors with
/// agent_not_idle there).
pub async fn read_agent_visible(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "agent.read",
        json!({"target": pane, "source": "visible", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

pub async fn send_agent_keys(socket: &str, pane: &str, keys: &[&str]) -> Res<()> {
    rpc(
        socket,
        "agent.send_keys",
        json!({"target": pane, "keys": keys}),
    )
    .await?;
    Ok(())
}
