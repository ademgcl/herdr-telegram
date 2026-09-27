use crate::{
    herdr::agents::list_agents,
    notifier::{observe_status, reconcile},
    state::AppState,
    types::Res,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::UnixStream,
};

pub async fn event_task(s: AppState) {
    let mut fails: u64 = 0;
    loop {
        let start = tokio::time::Instant::now();
        let failed = match run_stream(&s).await {
            Ok(reason) => {
                println!("[events] resubscribe ({reason})");
                false
            }
            Err(e) => {
                eprintln!(
                    "[events] stream error: {}",
                    crate::types::mask_home(&e.to_string())
                );
                true
            }
        };
        // Slow failures must still back off: resetting on any long cycle
        // retries a sick herdr every ~30s with a full reconcile each time.
        if failed || start.elapsed() < Duration::from_secs(10) {
            fails += 1;
        } else {
            fails = 0;
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
    let mut subs: Vec<Value> = agents
        .iter()
        .map(|a| json!({"type": "pane.agent_status_changed", "pane_id": a.pane}))
        .collect();
    // Birth events, UNSCOPED (no pane_id): a pane spawned from the herdr
    // CLI is not in `agents` yet, so a per-pane subscription can never
    // see it arrive. Without these, discovery waited for the 60s watchdog
    // — the reported "I thought it did not arrive". `pane.agent_detected`
    // is the precise one: it fires the moment a pane becomes an agent,
    // which is exactly when a topic must exist for it.
    for t in [
        "pane.created",
        "pane.agent_detected",
        "tab.created",
        "workspace.created",
    ] {
        subs.push(json!({ "type": t }));
    }

    let conn =
        match tokio::time::timeout(Duration::from_secs(30), UnixStream::connect(&s.cfg.socket))
            .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err("event stream connect timed out".into()),
        };
    let (reader, mut writer) = conn.into_split();
    // Bounded like connect/ack: a wedged herdr (accept but never drain)
    // must return to the backoff loop, never wedge the task forever.
    match tokio::time::timeout(Duration::from_secs(30), async {
        writer
            .write_all(
                json!({"id": "sub", "method": "events.subscribe", "params": {"subscriptions": subs}})
                    .to_string()
                    .as_bytes(),
            )
            .await?;
        writer.write_all(b"\n").await?;
        writer.flush().await
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err("event subscribe write timed out".into()),
    }

    let mut reader = BufReader::new(reader);
    let mut ack = String::new();
    // Bounded like the watcher stream: a hung ack must return to the
    // backoff loop, never wedge the task. Empty acks reject (blind loop).
    // Take-during-read (ctl parity): a rogue line never OOMs (64KB cap).
    match tokio::time::timeout(Duration::from_secs(30), async {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        let mut limited = (&mut reader).take(65_536);
        limited.read_line(&mut ack).await
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err("event subscribe ack timed out".into()),
    }
    if ack.trim().is_empty() {
        return Err("event subscribe got empty ack".into());
    }
    // Truncated ack (take() cap hit mid-line) fails parse below and would
    // read as "not rejected" → subscribed blind. Reject it so the backoff
    // loop resubscribes fresh (rpc/stream take-during-read parity).
    if ack.len() >= 65_536 && !ack.ends_with('\n') {
        return Err("event subscribe ack oversize".into());
    }
    // Parsed rejection only (see ack_rejected): success acks may carry
    // `"error": null`, which a substring match misreads as rejection.
    if super::rpc::ack_rejected(&ack) {
        return Err(format!("subscribe rejected: {}", ack.trim()).into());
    }

    loop {
        // Refresh bound: a pane spawned mid-cycle isn't subscribed until
        // the next resubscribe — 120s caps its event blindness (the
        // watchdog still samples it meanwhile).
        if started.elapsed() > Duration::from_secs(120) {
            return Ok("refresh");
        }
        let mut line = String::new();
        // Bounded (ctl take-during-read parity): rogue lines skip, never OOM.
        let n = match tokio::time::timeout(Duration::from_secs(90), async {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt};
            let mut limited = (&mut reader).take(262_144);
            limited.read_line(&mut line).await
        })
        .await
        {
            Err(_) => return Ok("idle timeout"),
            Ok(res) => res?,
        };
        if n == 0 {
            return Ok("connection closed");
        }
        // Truncation, not size: a complete line of exactly the cap
        // (payload + newline) is legal — only a missing trailing newline
        // proves take() cut mid-line (rpc/stream parity). Without the
        // guard a full event is misclassified as rogue and the drain eats
        // the next real event.
        if line.len() >= 262_144 && !line.ends_with('\n') {
            // `take` stopped mid-line: the remainder would else parse
            // as the next event (spurious status). Drain to the newline
            // first (bounded: a newline-free flood resubscribes fresh).
            use tokio::io::AsyncBufReadExt;
            let mut drained = 0usize;
            let flooded = loop {
                let mut tail = Vec::new();
                match tokio::time::timeout(Duration::from_secs(30), async {
                    use tokio::io::AsyncReadExt;
                    (&mut reader)
                        .take(262_144)
                        .read_until(b'\n', &mut tail)
                        .await
                })
                .await
                {
                    Ok(Ok(0)) => break false,
                    Ok(Ok(_)) => {
                        drained += tail.len();
                        if tail.ends_with(b"\n") || drained > 4_194_304 {
                            break drained > 4_194_304;
                        }
                    }
                    _ => break true,
                }
            };
            if flooded {
                return Ok("oversize event flood");
            }
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // NOTE: wire name is dotted ("pane.agent_status_changed", same as
        // the subscription type) — NOT underscored.
        let ev_name = ev["event"].as_str().unwrap_or("");
        if is_lifecycle_event(ev_name) {
            // Discovery nudge, not a reconcile: the main loop owns the
            // scan (single-flight), so this only wakes it.
            println!("[events] {ev_name} — waking reconcile");
            crate::state::wake_reconcile();
            continue;
        }
        if ev_name == "pane.agent_status_changed"
            && let Some((pane, status)) = parse_status_event(&ev)
        {
            // Paced reset owns topic lifecycle: card/debounce arms must
            // not fire into topics being deleted (429 storm + orphans).
            // Watchdog reconcile already degrades to reap-only during reset.
            if crate::handlers::reset::is_resetting() {
                continue;
            }
            println!("[events] {pane} → {status}");
            observe_status(s, pane, status, false, "event").await;
        }
    }
}

/// Pure: does this event mean "something was born"? Drives the
/// discovery nudge, so a typo here is a silent latency regression —
/// hence a test naming the exact set.
pub(crate) fn is_lifecycle_event(name: &str) -> bool {
    matches!(
        name,
        "pane.created" | "pane.agent_detected" | "tab.created" | "workspace.created"
    )
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
#[path = "events_tests.rs"]
mod tests;
