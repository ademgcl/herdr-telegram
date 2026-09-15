use crate::{
    herdr::client::list_agents,
    notifier::{observe_status, reconcile},
    state::AppState,
    types::Res,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

pub async fn event_task(s: AppState) {
    let mut fails: u64 = 0;
    loop {
        let start = tokio::time::Instant::now();
        match run_stream(&s).await {
            Ok(reason) => println!("[events] resubscribe ({reason})"),
            Err(e) => eprintln!("[events] stream error: {e}"),
        }
        if start.elapsed() < Duration::from_secs(10) {
            fails += 1;
        } else {
            fails = 1;
        }
        tokio::time::sleep(Duration::from_secs((5 * fails).min(60))).await;
        reconcile(&s, false, "reconnect").await;
    }
}

async fn run_stream(s: &AppState) -> Res<&'static str> {
    let started = tokio::time::Instant::now();
    let agents = list_agents(&s.cfg.socket)
        .await
        .map_err(|e| e.to_string())?;
    let subs: Vec<Value> = agents
        .iter()
        .map(|a| json!({"type": "pane.agent_status_changed", "pane_id": a.pane}))
        .collect();

    if subs.is_empty() {
        tokio::time::sleep(Duration::from_secs(20)).await;
        return Ok("no agents to watch");
    }

    let conn = UnixStream::connect(&s.cfg.socket).await?;
    let (reader, mut writer) = conn.into_split();
    writer
        .write_all(
            json!({"id": "sub", "method": "events.subscribe", "params": {"subscriptions": subs}})
                .to_string()
                .as_bytes(),
        )
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut ack = String::new();
    reader.read_line(&mut ack).await?;
    if ack.contains("\"error\"") {
        return Err(format!("subscribe rejected: {}", ack.trim()).into());
    }

    loop {
        if started.elapsed() > Duration::from_secs(300) {
            return Ok("refresh");
        }
        let mut line = String::new();
        let n = match tokio::time::timeout(Duration::from_secs(90), reader.read_line(&mut line))
            .await
        {
            Err(_) => return Ok("idle timeout"),
            Ok(res) => res?,
        };
        if n == 0 {
            return Ok("connection closed");
        }
        let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // NOTE: wire name is dotted ("pane.agent_status_changed", same as
        // the subscription type) — NOT underscored.
        if ev["event"].as_str() == Some("pane.agent_status_changed")
            && let Some((pane, status)) = parse_status_event(&ev)
        {
            println!("[events] {pane} → {status}");
            observe_status(s, pane, status, false, "event").await;
        }
    }
}

/// (pane, status) straight from the event payload. The event IS the
/// transition — re-fetching via agent.get here races herdr's own state
/// machine and records wrong/superseded statuses (stuck 🔄, invisible
/// working bursts), so the reported status is trusted as-is.
fn parse_status_event(ev: &Value) -> Option<(&str, &str)> {
    let pane = ev["data"]["pane_id"].as_str()?;
    let status = ev["data"]["agent_status"].as_str()?;
    Some((pane, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_event_wire_shape() {
        let ev: Value = serde_json::from_str(
            r#"{"data":{"agent":"opencode","agent_status":"working","pane_id":"wG:p2","workspace_id":"wG"},"event":"pane.agent_status_changed"}"#,
        )
        .unwrap();
        assert_eq!(parse_status_event(&ev), Some(("wG:p2", "working")));
    }

    #[test]
    fn test_parse_status_event_rejects_malformed() {
        let missing: Value = serde_json::from_str(
            r#"{"data":{"pane_id":"wG:p2"},"event":"pane.agent_status_changed"}"#,
        )
        .unwrap();
        assert_eq!(parse_status_event(&missing), None);
        let wrong_name: Value = serde_json::from_str(
            r#"{"data":{"agent_status":"idle","pane_id":"wG:p2"},"event":"pane_agent_status_changed"}"#,
        )
        .unwrap();
        // Caller matches the dotted name first; parser only reads data.
        assert_eq!(parse_status_event(&wrong_name), Some(("wG:p2", "idle")));
    }
}
