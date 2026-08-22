use std::time::Duration;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use crate::types::{AgentDetail, AgentRow, Res, WorkspaceInfo};

pub async fn rpc(socket: &str, method: &str, params: Value) -> Res<Value> {
    rpc_t(socket, method, params, 30).await
}

pub async fn rpc_t(socket: &str, method: &str, params: Value, timeout_secs: u64) -> Res<Value> {
    let fut = async {
        let mut conn = UnixStream::connect(socket).await?;
        let req = json!({"id": "tg", "method": method, "params": params});
        conn.write_all(format!("{req}\n").as_bytes()).await?;
        conn.flush().await?;
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let v: Value = serde_json::from_str(line.trim())?;
        if let Some(e) = v.get("error") {
            return Err(e["message"].as_str().unwrap_or("herdr error").to_string().into());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    };
    tokio::time::timeout(Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| format!("herdr {method} timed out"))?
}

pub async fn ping(socket: &str) -> Res<Value> {
    rpc(socket, "ping", json!({})).await
}

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

pub async fn read_pane_output(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "pane.read",
        json!({"pane_id": pane, "source": "recent_unwrapped", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

pub async fn list_workspaces(socket: &str) -> Res<Vec<WorkspaceInfo>> {
    let r = rpc(socket, "workspace.list", json!({})).await?;
    let mut out = Vec::new();
    for w in r["workspaces"].as_array().cloned().unwrap_or_default() {
        out.push(WorkspaceInfo {
            id: w["workspace_id"].as_str().unwrap_or("?").into(),
            label: w["label"].as_str().unwrap_or("?").into(),
            number: w["number"].as_u64().unwrap_or(0),
        });
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
    let pane = tab["root_pane"]["pane_id"].as_str().unwrap_or("").to_string();

    let name = format!(
        "tg-{kind}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );

    rpc_t(
        socket,
        "agent.start",
        json!({"name": name, "kind": kind, "pane_id": pane}),
        90,
    )
    .await?;

    let detail = get_agent(socket, &pane).await?;
    Ok(detail.into())
}

pub async fn send_agent_keys(socket: &str, pane: &str, keys: &[&str]) -> Res<()> {
    rpc(socket, "agent.send_keys", json!({"target": pane, "keys": keys})).await?;
    Ok(())
}

pub async fn send_pane_text(socket: &str, pane: &str, text: &str) -> Res<()> {
    rpc_t(socket, "pane.send_text", json!({"pane_id": pane, "text": text}), 30).await?;
    rpc(socket, "pane.send_keys", json!({"pane_id": pane, "keys": ["enter"]})).await?;
    Ok(())
}
