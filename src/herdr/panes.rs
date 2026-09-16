use super::labels::{PaneFacts, pane_facts};
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

/// Numeric suffix of a pane id (`wJ:p12` → 12): orders panes within a
/// workspace so callers can find the root pane. Unparseable → MAX
/// (sorts last, never picked over a real pane).
pub fn pane_num(id: &str) -> u64 {
    id.rsplit(":p")
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Lowest pane (`p1`) in `ws`: `workspace.create` already ships a root
/// pane, so fresh-space flows must reuse it — a fresh `tab.create`
/// would orphan p1 and hang the topic on p2. Pure so tests cover it
/// without I/O. Only for just-created workspaces: never call with an
/// existing ws (it would hijack a live pane — see `first_shell_pane`).
/// Empty `ws` never matches (pane rows default a missing workspace to
/// `""`, which must not be attachable). None when nothing parses:
/// an all-garbage tie must fall back, not pick at random.
pub fn pick_first_pane(facts: &HashMap<String, PaneFacts>, ws: &str) -> Option<String> {
    if ws.is_empty() {
        return None;
    }
    let best = facts
        .iter()
        .filter(|(_, f)| f.ws == ws)
        .map(|(id, _)| id)
        .min_by_key(|id| pane_num(id))?;
    if pane_num(best) == u64::MAX {
        return None;
    }
    Some(best.clone())
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
            eprintln!("[herdr] pane.list failed, skipping reuse: {e}");
            None
        }
    }
}

/// Shell check: `agent.get` succeeds only on agent panes (shells are
/// invisible to it — same shape as the DM shell fallback). Fail-open
/// (Err → shell) is safe HERE because callers only probe just-created
/// workspaces: the pane is ours by construction, and a pane that died
/// mid-flight self-heals via the reconcile watchdog (dead topic ages
/// out; commands fail visibly until then).
pub async fn is_shell_pane(socket: &str, pane: &str) -> bool {
    super::agents::get_agent(socket, pane).await.is_err()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::labels::parse_facts;

    fn facts(json: &str) -> HashMap<String, PaneFacts> {
        parse_facts(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn test_pick_first_pane_prefers_p1() {
        let m = facts(
            r#"{"panes": [
                {"pane_id": "wJ:p2", "workspace_id": "wJ"},
                {"pane_id": "wJ:p1", "workspace_id": "wJ"}]}"#,
        );
        assert_eq!(pick_first_pane(&m, "wJ"), Some("wJ:p1".to_string()));
    }

    #[test]
    fn test_pick_first_pane_ignores_other_ws() {
        let m = facts(
            r#"{"panes": [
                {"pane_id": "w8:p1", "workspace_id": "w8"},
                {"pane_id": "wJ:p3", "workspace_id": "wJ"}]}"#,
        );
        assert_eq!(pick_first_pane(&m, "wJ"), Some("wJ:p3".to_string()));
    }

    #[test]
    fn test_pick_first_pane_none_when_empty() {
        let m = facts(r#"{"panes": [{"pane_id": "w8:p1", "workspace_id": "w8"}]}"#);
        assert_eq!(pick_first_pane(&m, "wJ"), None);
    }

    #[test]
    fn test_pick_first_pane_rejects_empty_ws() {
        // Rows with a missing workspace default to "": "" must never
        // match, or an empty id could attach cross-workspace.
        let m = facts(r#"{"panes": [{"pane_id": "w8:p1"}]}"#);
        assert_eq!(pick_first_pane(&m, ""), None);
    }

    #[test]
    fn test_pick_first_pane_none_when_unparseable() {
        let m = facts(r#"{"panes": [{"pane_id": "bogus", "workspace_id": "wJ"}]}"#);
        assert_eq!(pick_first_pane(&m, "wJ"), None);
    }

    #[test]
    fn test_pane_num_orders_numerically() {
        assert!(pane_num("wJ:p2") < pane_num("wJ:p12"));
        assert_eq!(pane_num("bogus"), u64::MAX);
        assert_eq!(pane_num("wJ:p"), u64::MAX);
    }
}
