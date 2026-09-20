use super::labels::pane_facts;
pub use super::pane_select::{best_split_direction, parse_layout, pick_first_pane};
use super::rpc::{rpc, rpc_t};
use crate::types::Res;
use serde_json::json;
use std::collections::HashMap;

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
        // Confirmed death is not a source problem: falling back to the
        // viewport would serve a shell-reused name's output as the dead
        // pane's (delaying retire to the next hygiene cycle).
        Err(e) if super::rpc::is_not_found(&e.to_string()) => Err(e),
        // A timeout already spent the full RPC budget: falling back
        // doubles a sick-herdr read to ~60s on the settle path. Bound
        // it — the next tick retries.
        Err(e) if super::rpc::is_timeout(&e.to_string()) => Err(e),
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
/// Malformed replies error (workspaces parity): empty ids must not flow
/// downstream fail-open as ghost panes.
pub async fn create_tab(socket: &str, ws: &str) -> Res<String> {
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

/// Probe a just-created workspace's root shell pane, retrying while
/// `workspace.create` materializes it (a single probe races it and
/// falls back to `tab.create` — the original orphan bug). Holds no
/// locks (pure RPC probes). None → caller must `tab.create`.
pub async fn await_fresh_root(socket: &str, ws: &str) -> Option<String> {
    for attempt in 0..4 {
        if let Some(p) = first_shell_pane(socket, ws).await {
            return Some(p);
        }
        if attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    None
}

/// Lowest live pane in `ws`, if visible yet. None → caller falls back
/// to `tab.create` (old behavior, still functional).
pub async fn first_pane_in_ws(socket: &str, ws: &str) -> Option<String> {
    match pane_facts(socket).await {
        Ok(m) => pick_first_pane(&m, ws),
        Err(e) => {
            eprintln!(
                "[herdr] pane.list failed, skipping reuse: {}",
                crate::types::mask_home(&e.to_string())
            );
            None
        }
    }
}

/// Shell check: `agent.get` succeeds only on agent panes (shells are
/// invisible to it — same shape as the DM shell fallback). Fail-closed:
/// only a confirmed-dead lookup reads as shell — a transient (timeout,
/// herdr down) reads as NOT shell so `first_shell_pane` falls back to
/// `tab.create` instead of hijacking a live agent pane on reuse.
pub async fn is_shell_pane(socket: &str, pane: &str) -> bool {
    match super::agents::get_agent(socket, pane).await {
        Ok(_) => false,
        Err(e) => super::rpc::is_not_found(&e.to_string()),
    }
}

/// True when the shell sits at a prompt (the foreground group is the
/// shell itself): the completion signal for shell settle (see
/// `await_shell_settle`). A running command owns the foreground group
/// instead — screen stability alone false-settles slow commands whose
/// typed echo sits unchanged for seconds before output arrives. None
/// on any RPC/parse failure: callers degrade to a longer timing-only
/// bar, never guess from unknown.
pub async fn is_shell_idle(socket: &str, pane: &str) -> Option<bool> {
    let r = rpc_t(
        socket,
        "pane.process_info",
        json!({"pane_id": pane}),
        PROCESS_INFO_TIMEOUT_SECS,
    )
    .await
    .ok()?;
    parse_shell_idle(&r)
}

/// Bound for the settle busy-probe: process info is a local table read
/// (milliseconds) — a slow one degrades to the timing bar fast, never
/// holds settle rounds hostage behind the 30s default RPC timeout.
const PROCESS_INFO_TIMEOUT_SECS: u64 = 10;

/// Pure parse (tests cover the shape without I/O): idle only when the
/// foreground group AND every listed pid agree it is the shell. Foreign
/// anywhere reads busy (the list lags the group across fork/exec);
/// empty list defers to the group id (missing too = unknown, never
/// idle); all-shell without group id = unknown (builtins stay invisible
/// — accepted residual, quiet long ones settle on the timing bar).
pub fn parse_shell_idle(v: &serde_json::Value) -> Option<bool> {
    let info = v.get("process_info")?;
    let shell = info.get("shell_pid")?.as_u64()?;
    let pgid = info
        .get("foreground_process_group_id")
        .and_then(serde_json::Value::as_u64);
    let fg = info.get("foreground_processes")?.as_array()?;
    for p in fg {
        match p.get("pid").and_then(serde_json::Value::as_u64) {
            Some(pid) if pid == shell => {}
            Some(_) => return Some(false),
            None => return None,
        }
    }
    if fg.is_empty() {
        // No listed processes: the group id is the only signal.
        return pgid.map(|g| g == shell);
    }
    // Non-empty list needs the group id too: builtins and old servers
    // report an all-shell list with no confirmation — unknown degrades
    // to the longer timing bar, never a fast idle verdict.
    pgid.map(|g| g == shell)
}

/// Lowest pane in a JUST-CREATED `ws`, verified shell (never an
/// agent). None → caller must `tab.create` instead of hijacking.
pub async fn first_shell_pane(socket: &str, ws: &str) -> Option<String> {
    let pane = first_pane_in_ws(socket, ws).await?;
    if is_shell_pane(socket, &pane).await {
        Some(pane)
    } else {
        eprintln!("[herdr] root pane {pane} hosts an agent, skipping reuse");
        None
    }
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

/// Live tab geometry: pane id → (width, height) cells from
/// `pane.layout`. Powers bare-`/split` direction picking.
pub async fn pane_layout(socket: &str, tab_id: &str) -> Res<HashMap<String, (u64, u64)>> {
    let r = rpc(socket, "pane.layout", json!({"tab_id": tab_id})).await?;
    Ok(parse_layout(&r))
}

#[cfg(test)]
#[path = "panes_tests.rs"]
mod tests;
