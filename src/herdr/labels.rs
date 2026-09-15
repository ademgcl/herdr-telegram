//! Pane display labels + workspace placement: the user-set name synced
//! 1:1 with forum topic titles (`pane.rename` sets it, `pane.list`
//! serves it, absent when cleared). Read-only except via `rename_pane`.
use crate::{herdr::rpc::rpc, types::Res};
use serde_json::{Value, json};
use std::collections::HashMap;

/// What title sync needs per pane: display label (if any) + workspace.
pub struct PaneFacts {
    pub label: Option<String>,
    pub ws: String,
}

/// One `pane.list` per call — the reconcile watchdog's 60s tick is the
/// only periodic caller.
pub async fn pane_facts(socket: &str) -> Res<HashMap<String, PaneFacts>> {
    let r = rpc(socket, "pane.list", json!({})).await?;
    Ok(parse_facts(&r))
}

/// Pure parse so tests cover the shape without I/O.
pub fn parse_facts(v: &Value) -> HashMap<String, PaneFacts> {
    let mut out = HashMap::new();
    for p in v["panes"].as_array().cloned().unwrap_or_default() {
        if let Some(id) = p["pane_id"].as_str() {
            out.insert(
                id.to_string(),
                PaneFacts {
                    label: p["label"].as_str().map(|s| s.to_string()),
                    ws: p["workspace_id"].as_str().unwrap_or("").to_string(),
                },
            );
        }
    }
    out
}

/// Set (or with `None`, clear) a pane's display label.
pub async fn rename_pane(socket: &str, pane: &str, label: Option<&str>) -> Res<()> {
    rpc(
        socket,
        "pane.rename",
        json!({"pane_id": pane, "label": label}),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_facts() {
        let v: Value = serde_json::from_str(
            r#"{"panes": [{"pane_id": "w8:p1", "label": "api", "workspace_id": "w8"}, {"pane_id": "w8:p3", "workspace_id": "w8"}]}"#,
        )
        .unwrap();
        let m = parse_facts(&v);
        assert_eq!(m["w8:p1"].label, Some("api".to_string()));
        assert_eq!(m["w8:p1"].ws, "w8");
        assert_eq!(m["w8:p3"].label, None);
    }
}
