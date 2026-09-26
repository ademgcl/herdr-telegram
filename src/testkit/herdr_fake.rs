//! Fake herdr JSON-RPC server for e2e tests: records submits, serves
//! a scriptable agent row/status/screen. Tests drive a real turn —
//! prompt → submit → working → settle → final — through the production
//! watcher, so the edge cases are exercised end to end.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

/// Scripted agent state the test mutates between assertions.
#[derive(Default)]
pub struct FakeHerdr {
    /// Prompts the bot actually delivered (`agent.prompt`).
    pub submits: Mutex<Vec<String>>,
    /// `agent.get` status field ("working" / "idle" / "blocked" / …).
    pub status: Mutex<String>,
    /// `agent.read` output, newest last per source.
    pub screen: Mutex<Vec<String>>,
    /// `pane.list` result.
    pub panes: Mutex<Vec<String>>,
    /// Fail the next `agent.prompt` with this description.
    pub submit_fault: Mutex<Option<String>>,
}

impl FakeHerdr {
    pub fn submitted(&self) -> Vec<String> {
        self.submits.lock().unwrap().clone()
    }
    pub fn submit_count(&self) -> usize {
        self.submits.lock().unwrap().len()
    }
    pub fn set_status(&self, s: &str) {
        *self.status.lock().unwrap() = s.to_string();
    }
    pub fn set_screen(&self, lines: &[&str]) {
        *self.screen.lock().unwrap() = lines.iter().map(|l| l.to_string()).collect();
    }
    pub fn set_screen_owned(&self, lines: Vec<String>) {
        *self.screen.lock().unwrap() = lines;
    }
    pub fn fail_next_submit(&self, description: &str) {
        *self.submit_fault.lock().unwrap() = Some(description.to_string());
    }
    fn take_submit_fault(&self) -> Option<String> {
        self.submit_fault.lock().unwrap().take()
    }
}

/// Wire shape of an agent row (agent.get / agent.list): herdr keys are
/// `pane_id`/`agent_status`/`workspace_id`, NOT the bot's parsed names.
fn agent_row(pane: &str, status: &str) -> Value {
    json!({
        "pane_id": pane,
        "agent": "opencode",
        "agent_status": status,
        "workspace_id": "w1",
        "tab_id": "w1:t1",
        "cwd": "/tmp/e2e",
        "foreground_cwd": "/tmp/e2e",
        "terminal_title_stripped": "e2e",
        "label": "",
    })
}

/// Bind the fake on a temp socket path; returns the shared state + path.
pub async fn start(dir: &std::path::Path) -> (Arc<FakeHerdr>, String) {
    let path = dir.join("herdr-fake.sock");
    let _ = std::fs::remove_file(&path);
    let fake = Arc::new(FakeHerdr {
        status: Mutex::new("idle".to_string()),
        panes: Mutex::new(vec!["w1:p1".to_string()]),
        ..Default::default()
    });
    let listener = UnixListener::bind(&path).expect("fake herdr bind");
    let served = fake.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let f = served.clone();
            tokio::spawn(async move {
                let _ = serve_conn(sock, f).await;
            });
        }
    });
    (fake, path.to_string_lossy().to_string())
}

async fn serve_conn(sock: UnixStream, fake: Arc<FakeHerdr>) -> std::io::Result<()> {
    let (read, mut write) = sock.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(req) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let id = req["id"].as_str().unwrap_or("").to_string();
        let method = req["method"].as_str().unwrap_or("").to_string();
        let params = req["params"].clone();
        let resp = respond(&fake, &id, &method, &params);
        let out = resp.to_string();
        write.write_all(out.as_bytes()).await?;
        write.write_all(b"\n").await?;
        write.flush().await?;
    }
    Ok(())
}

/// Full JSON-RPC reply: herdr reports failures as an `error` object,
/// so a scripted submit fault must travel that way (a `result: null`
/// would read as success to the bot). The request id is echoed so
/// correlation matches the real socket.
fn respond(fake: &FakeHerdr, id: &str, method: &str, params: &Value) -> Value {
    let (result, err) = dispatch(fake, method, params);
    match err {
        Some(msg) => json!({"id": id, "error": {"code": -32000, "message": msg}}),
        None => json!({"id": id, "result": result}),
    }
}

fn dispatch(fake: &FakeHerdr, method: &str, params: &Value) -> (Value, Option<String>) {
    let pane = params["target"].as_str().unwrap_or("w1:p1").to_string();
    let status = fake.status.lock().unwrap().clone();
    let out = match method {
        "agent.prompt" => {
            if let Some(e) = fake.take_submit_fault() {
                return (Value::Null, Some(e));
            }
            let text = params["text"].as_str().unwrap_or("").to_string();
            fake.submits.lock().unwrap().push(text);
            json!({"ok": true})
        }
        // Envelopes mirror the real socket: `{"agent": row}`,
        // `{"agents": [rows]}`, `{"read": {"text": …}}`.
        "agent.get" => json!({"agent": agent_row(&pane, &status)}),
        "agent.list" => json!({"agents": [agent_row(&pane, &status)]}),
        "agent.read" => {
            let lines = fake.screen.lock().unwrap().clone();
            json!({"read": {"text": lines.join("\n")}})
        }
        "pane.list" => {
            let panes = fake.panes.lock().unwrap().clone();
            json!({"panes": panes
                .iter()
                .map(|p| json!({"pane_id": p, "tab_id": "w1:t1", "ws": "w1",
                                 "label": "", "focused": true}))
                .collect::<Vec<_>>()})
        }
        "workspace.list" => json!([{"id": "w1", "number": 1, "label": "main"}]),
        "events.subscribe" => json!({"ok": true}),
        // Unknown methods answer empty rather than erroring: the bot
        // treats unknown reads as outage, which would mask the case
        // under test.
        _ => json!({}),
    };
    (out, None)
}
