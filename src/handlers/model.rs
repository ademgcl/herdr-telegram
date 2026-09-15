/// Opencode model picker (`/model`): current model, free-Zen shortlist,
/// and live switching of the ACTIVE session.
///
/// herdr has no model API (agent.start/prompt take no model), and config
/// files are out of bounds — so this drives the TUI `/models` picker the
/// same way a local user would: open, type to filter, Enter to select,
/// verify in the footer. Every step is confirmed; failures esc out and
/// report instead of blind-switching.
///
/// Default view is the 7-item free-Zen shortlist — never a full picker
/// dump. Anything else goes through `/model <search>` (e.g. `/model gpt`).
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

use super::model_parse::{FREE_ZEN, footer_model, norm};
use super::model_scan::picker_hit;
use crate::{
    herdr::client::{get_agent, read_screen_visible, send_agent_keys, type_pane_text},
    state::AppState,
};

/// `/model <search>` filter: prefer Zen (free) unless the query already
/// names a provider — avoids landing on same-name OpenRouter rows.
pub fn search_filter(q: &str) -> String {
    let low = q.to_lowercase();
    let providerish = [
        "zen",
        "openrouter",
        "anthropic",
        "openai",
        "google",
        "opencode",
    ]
    .iter()
    .any(|p| low.contains(p));
    if providerish {
        q.to_lowercase()
    } else {
        format!("{} zen", q.to_lowercase())
    }
}
/// Default `/model` card: current footer model + numbered free-Zen
/// shortlist. Pure — tested via model_parse.
pub fn model_card_text(current: Option<&str>, pane: &str, kind: &str) -> String {
    if kind != "opencode" {
        return format!(
            "🤖 model [{pane}]\nkind `{kind}` has no picker — model switching is opencode-only."
        );
    }
    let mut text = format!(
        "🤖 model [{pane}]\n▶ now using: {}\n\nfree Zen — tap to switch:",
        current.unwrap_or("(couldn't read — agent may be busy)")
    );
    for (i, (name, _, _)) in FREE_ZEN.iter().enumerate() {
        text.push_str(&format!("\n{}. {name}", i + 1));
    }
    text.push_str("\n\nor `/model <search>` for anything else (e.g. `/model gpt`).");
    text
}

pub fn model_kb(pane: &str) -> Value {
    let mut rows = Vec::new();
    for (i, (_, _, short)) in FREE_ZEN.iter().enumerate() {
        if i % 2 == 0 {
            rows.push(Vec::new());
        }
        rows.last_mut().expect("row pushed on even i").push(json!({
            "text": *short,
            // model ids hold no ':' — pane goes last so splitn keeps it whole.
            "callback_data": format!("M:{i}:{pane}"),
        }));
    }
    Value::Array(rows.into_iter().map(Value::Array).collect())
}

/// Show the model card for `pane` (current + free-Zen buttons).
pub async fn show_model(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let kind = get_agent(&s.cfg.socket, pane)
        .await
        .map(|a| a.kind)
        .unwrap_or_else(|_| "?".into());
    let cur = current_model(s, pane).await;
    let text = model_card_text(cur.as_deref(), pane, &kind);
    let kb = if kind == "opencode" {
        Some(model_kb(pane))
    } else {
        None
    };
    let mid = s.tg.send_msg(chat, thread, &text, kb).await;
    s.remember(chat, mid, pane).await;
    s.set_focus(pane).await;
}

/// Switch `pane` to `filter`/`marker`, narrating progress into the chat.
pub async fn switch_by_filter(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    filter: &str,
    marker: &str,
) {
    s.set_focus(pane).await;
    s.tg.send_msg(
        chat,
        thread,
        &format!("⏳ switching {pane} → `{marker}`…"),
        None,
    )
    .await;
    match switch_model(s, pane, filter, marker).await {
        // Set means set: plain confirmation, NO picker keyboard back —
        // re-offering options after success only confuses.
        Ok(footer) => {
            let mid =
                s.tg.send_msg(
                    chat,
                    thread,
                    &format!("✅ model set: `{footer}`\n[{pane}]"),
                    None,
                )
                .await;
            s.remember(chat, mid, pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ switch failed: {e}"), None)
                .await;
        }
    }
}

/// Open the picker; Err (after esc) when it didn't open.
async fn open_picker(s: &AppState, pane: &str) -> Result<Vec<String>, String> {
    // Close anything already open first (esc on plain input is harmless).
    send_agent_keys(&s.cfg.socket, pane, &["esc"])
        .await
        .map_err(|e| e.to_string())?;
    sleep(Duration::from_millis(800)).await;
    type_pane_text(&s.cfg.socket, pane, "/models")
        .await
        .map_err(|e| e.to_string())?;
    send_agent_keys(&s.cfg.socket, pane, &["enter"])
        .await
        .map_err(|e| e.to_string())?;
    sleep(Duration::from_secs(2)).await;
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    if screen.iter().any(|l| l.contains("Select model")) {
        Ok(screen)
    } else {
        let _ = send_agent_keys(&s.cfg.socket, pane, &["esc"]).await;
        Err("model picker did not open (agent busy?)".to_string())
    }
}

async fn close_picker(s: &AppState, pane: &str) {
    let _ = send_agent_keys(&s.cfg.socket, pane, &["esc"]).await;
}

/// Switch the LIVE session's model: open → filter → Enter → verify in
/// the footer → esc safety. Returns the new footer label.
pub async fn switch_model(
    s: &AppState,
    pane: &str,
    filter: &str,
    marker: &str,
) -> Result<String, String> {
    if s.modelop.lock().await.contains(pane) {
        return Err("a model switch is already running here".to_string());
    }
    s.modelop.lock().await.insert(pane.to_string());
    let r = switch_inner(s, pane, filter, marker).await;
    s.modelop.lock().await.remove(pane);
    r
}

async fn switch_inner(
    s: &AppState,
    pane: &str,
    filter: &str,
    marker: &str,
) -> Result<String, String> {
    let agent = get_agent(&s.cfg.socket, pane)
        .await
        .map_err(|e| e.to_string())?;
    if agent.kind != "opencode" {
        return Err(format!(
            "model switching is for opencode agents (this is {})",
            agent.kind
        ));
    }
    if !matches!(agent.status.as_str(), "idle" | "done") {
        return Err(format!("agent is {} — try when idle", agent.status));
    }
    open_picker(s, pane).await?;
    // Narrow, then confirm the target is actually listed.
    type_pane_text(&s.cfg.socket, pane, filter)
        .await
        .map_err(|e| e.to_string())?;
    sleep(Duration::from_secs(2)).await;
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    // Bleed-proof substring check (NOT the column parse): background chat
    // text shares terminal rows with picker rows and shifts columns.
    if !picker_hit(&screen, filter, marker) {
        close_picker(s, pane).await;
        return Err(format!(
            "no model matches '{filter}' — stale card? run /model again for a fresh list"
        ));
    }
    // Re-check idleness: typing must never leak into a working session.
    let busy = get_agent(&s.cfg.socket, pane)
        .await
        .map(|a| !matches!(a.status.as_str(), "idle" | "done"))
        .unwrap_or(true);
    if busy {
        close_picker(s, pane).await;
        return Err("agent got busy — try when idle".to_string());
    }
    send_agent_keys(&s.cfg.socket, pane, &["enter"])
        .await
        .map_err(|e| e.to_string())?;
    // The footer lags the actual switch (model reload) — poll it instead
    // of trusting a single read, or fast switches report a false
    // "unverified" while showing the old footer.
    let mut footer = String::new();
    for _ in 0..4 {
        sleep(Duration::from_secs(3)).await;
        let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
        footer = footer_model(&screen).unwrap_or_default();
        if norm(&footer).contains(&norm(marker)) {
            close_picker(s, pane).await;
            return Ok(footer);
        }
    }
    close_picker(s, pane).await;
    Err(format!("switch unverified (footer shows '{footer}')"))
}

/// Current model label, or None when unreadable. Tries a deeper viewport
/// as fallback — on busy screens the footer can sit below 40 lines.
pub async fn current_model(s: &AppState, pane: &str) -> Option<String> {
    let screen = read_screen_visible(&s.cfg.socket, pane, 40).await;
    if let Some(m) = footer_model(&screen) {
        return Some(m);
    }
    let screen = read_screen_visible(&s.cfg.socket, pane, 120).await;
    footer_model(&screen)
}

#[cfg(test)]
mod tests {
    use super::super::model_parse::free_tap;
    use super::*;

    #[test]
    fn test_search_filter_prefers_zen() {
        assert_eq!(search_filter("gpt"), "gpt zen");
        assert_eq!(search_filter("gpt openrouter"), "gpt openrouter");
        assert_eq!(search_filter("Ling Flash"), "ling flash zen");
    }

    #[test]
    fn test_free_tap_reexport() {
        assert!(free_tap(0).is_some());
        assert!(free_tap(99).is_none());
    }

    #[test]
    fn test_model_kb_shape() {
        let kb = model_kb("w1:p1");
        // 7 free models, 2 per row → 4 rows; pane survives splitn(3).
        assert_eq!(kb.as_array().unwrap().len(), 4);
        let data = kb[0][0]["callback_data"].as_str().unwrap();
        assert_eq!(
            data.splitn(3, ':').collect::<Vec<_>>(),
            vec!["M", "0", "w1:p1"]
        );
        // Button labels are the shorts — full names never fit.
        assert_eq!(kb[0][0]["text"].as_str().unwrap(), "Big Pickle");
        assert_eq!(kb[0][1]["text"].as_str().unwrap(), "Muse Spark 1.3");
        assert!(
            kb.as_array()
                .unwrap()
                .iter()
                .flat_map(|r| r.as_array().unwrap())
                .all(|b| { b["text"].as_str().unwrap().chars().count() <= 20 })
        );
    }

    #[test]
    fn test_non_opencode_card() {
        let t = model_card_text(Some("x"), "w1:p1", "claude");
        assert!(t.contains("opencode-only"));
    }
}
