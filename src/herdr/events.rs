use std::time::Duration;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use crate::{
    herdr::client::{get_agent, list_agents},
    notifier::{observe_status, reconcile},
    state::AppState,
    types::Res,
};

pub async fn event_task(s: AppState) {
    loop {
        match run_stream(&s).await {
            Ok(reason) => println!("[events] resubscribe ({reason})"),
            Err(e) => eprintln!("[events] stream error: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        reconcile(&s, false, "reconnect").await;
    }
}

async fn run_stream(s: &AppState) -> Res<&'static str> {
    let agents = list_agents(&s.cfg.socket).await.map_err(|e| e.to_string())?;
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
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok("connection closed");
        }
        let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else { continue };
        // NOTE: wire name is dotted ("pane.agent_status_changed", same as
        // the subscription type) — NOT underscored.
        if ev["event"].as_str() == Some("pane.agent_status_changed")
            && let Some(pane) = ev["data"]["pane_id"].as_str()
        {
            println!("[events] {pane} status event");
            let status = match get_agent(&s.cfg.socket, pane).await {
                Ok(a) => a.status,
                Err(e) => {
                    eprintln!("[events] agent.get failed for {pane}: {e}");
                    continue;
                }
            };
            observe_status(s, pane, &status, false, "event").await;
        }
    }
}
