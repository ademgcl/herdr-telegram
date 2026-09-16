//! Pane + tab display labels + workspace placement: the user-set tab
//! name is the topic title source (`tab.rename` sets it, `tab.list`
//! serves it). Pane labels (`pane.rename` / `pane.list`) are legacy
//! bot-written friendly defaults, kept only as fallback for split tabs.
//! Read-only except via `rename_pane` / `rename_tab`.
use crate::{herdr::rpc::rpc, types::Res};
use serde_json::{Value, json};
use std::collections::HashMap;

/// What title sync needs per pane: pane label (if any) + workspace + tab.
pub struct PaneFacts {
    pub label: Option<String>,
    pub ws: String,
    pub tab_id: String,
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
    if let Some(arr) = v["panes"].as_array() {
        for p in arr {
            if let Some(id) = p["pane_id"].as_str() {
                out.insert(
                    id.to_string(),
                    PaneFacts {
                        // Cleared labels serve as "": treat as unset or title
                        // sync renames the topic to "" (Telegram 400) forever.
                        label: p["label"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string()),
                        ws: p["workspace_id"].as_str().unwrap_or("").to_string(),
                        tab_id: p["tab_id"].as_str().unwrap_or("").to_string(),
                    },
                );
            }
        }
    }
    out
}

/// One `tab.list` per call: user-visible tab names (the rename target
/// users actually edit in herdr UI). Returns tab_id -> label.
pub async fn tab_labels(socket: &str) -> Res<HashMap<String, String>> {
    let r = rpc(socket, "tab.list", json!({})).await?;
    Ok(parse_tab_labels(&r))
}

/// Pure parse so tests cover the shape without I/O.
pub fn parse_tab_labels(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(arr) = v["tabs"].as_array() {
        for t in arr {
            if let Some(id) = t["tab_id"].as_str()
                && let Some(label) = t["label"].as_str().filter(|s| !s.is_empty())
            {
                out.insert(id.to_string(), label.to_string());
            }
        }
    }
    out
}

/// Set a tab's user-visible name (what the herdr tab bar shows).
pub async fn rename_tab(socket: &str, tab_id: &str, label: &str) -> Res<()> {
    rpc(socket, "tab.rename", json!({"tab_id": tab_id, "label": label})).await?;
    Ok(())
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
            r#"{"panes": [{"pane_id": "w8:p1", "label": "api", "workspace_id": "w8", "tab_id": "w8:t1"}, {"pane_id": "w8:p3", "workspace_id": "w8"}]}"#,
        )
        .unwrap();
        let m = parse_facts(&v);
        assert_eq!(m["w8:p1"].label, Some("api".to_string()));
        assert_eq!(m["w8:p1"].ws, "w8");
        assert_eq!(m["w8:p1"].tab_id, "w8:t1");
        assert_eq!(m["w8:p3"].label, None);
    }

    #[test]
    fn test_parse_facts_cleared_label_is_unset() {
        let v: Value = serde_json::from_str(
            r#"{"panes": [{"pane_id": "w8:p1", "label": "", "workspace_id": "w8"}]}"#,
        )
        .unwrap();
        assert_eq!(parse_facts(&v)["w8:p1"].label, None);
    }

    #[test]
    fn test_parse_tab_labels() {
        let v: Value = serde_json::from_str(
            r#"{"tabs": [{"tab_id": "wH:t5", "label": "agy_gelistirme"}, {"tab_id": "wH:t4", "label": "console"}, {"tab_id": "wH:t9", "label": ""}]}"#,
        )
        .unwrap();
        let m = parse_tab_labels(&v);
        assert_eq!(m["wH:t5"], "agy_gelistirme");
        assert_eq!(m["wH:t4"], "console");
        assert!(!m.contains_key("wH:t9"));
    }
}
