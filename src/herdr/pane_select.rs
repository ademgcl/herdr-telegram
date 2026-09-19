use super::labels::PaneFacts;
use std::collections::HashMap;

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

/// Pure parse so tests cover the shape without I/O. Missing rects
/// never block a split (caller falls back to `right`).
pub fn parse_layout(v: &serde_json::Value) -> HashMap<String, (u64, u64)> {
    let layout = v.get("layout").unwrap_or(v);
    let mut out = HashMap::new();
    if let Some(arr) = layout["panes"].as_array() {
        for p in arr {
            if let (Some(id), Some(w), Some(h)) = (
                p["pane_id"].as_str(),
                p["rect"]["width"].as_u64(),
                p["rect"]["height"].as_u64(),
            ) {
                out.insert(id.to_string(), (w, h));
            }
        }
    }
    out
}

/// Split along the longer axis (wide → right, tall → down); ties go
/// right (the historic bare-`/split` default). Pure so unit-tested.
pub fn best_split_direction(width: u64, height: u64) -> &'static str {
    if width >= height { "right" } else { "down" }
}
