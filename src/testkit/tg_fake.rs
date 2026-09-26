//! Fake Telegram API for e2e tests: records every call the bot makes
//! and serves scriptable failures. This is the seam that was missing —
//! every user-visible bug (missing instant message, doubled finals,
//! dead-slot retries) happened between the bot and Telegram, where the
//! unit tests could not see it.

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// One recorded API call: method + request body + the message id the
/// fake assigned (sendMessage returns it — the request has none).
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub body: Value,
    pub assigned_id: Option<i64>,
}

impl Call {
    /// Message text the bot sent (sendMessage / editMessageText).
    pub fn text(&self) -> String {
        self.body["text"].as_str().unwrap_or("").to_string()
    }
    /// Message id the bot targeted (edit/delete carry it in the body).
    pub fn message_id(&self) -> i64 {
        self.body["message_id"].as_i64().unwrap_or(0)
    }
    /// Id this send created (None for non-sends / faults).
    pub fn sent_id(&self) -> i64 {
        self.assigned_id.unwrap_or(0)
    }
}

/// Shared fake state: call log, per-(chat,method) scripted faults, id
/// counter. ONE fake serves the whole test binary — per-test isolation
/// is by chat id, so e2e cases run in parallel without stealing each
/// other's endpoint (a per-harness endpoint would need a process-global
/// env var, which races exactly like that).
#[derive(Default)]
pub struct FakeTg {
    pub calls: Mutex<Vec<Call>>,
    faults: Mutex<HashMap<(i64, String), Vec<String>>>,
    next_id: Mutex<i64>,
}

impl FakeTg {
    /// Calls of one method for one chat, in order.
    pub fn calls_of(&self, chat: i64, method: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == method && c.chat_id() == chat)
            .cloned()
            .collect()
    }
    /// Every text the bot pushed to one chat (sends + edits).
    pub fn sent_texts(&self, chat: i64) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| {
                c.chat_id() == chat && (c.method == "sendMessage" || c.method == "editMessageText")
            })
            .map(|c| c.text())
            .collect()
    }
    pub fn sent_count(&self, chat: i64, method: &str) -> usize {
        self.calls_of(chat, method).len()
    }
    /// Queue a failure for the next `method` call in `chat` (other
    /// chats are untouched — parallel tests stay independent).
    pub fn fault_next(&self, chat: i64, method: &str, description: &str) {
        self.faults
            .lock()
            .unwrap()
            .entry((chat, method.to_string()))
            .or_default()
            .push(description.to_string());
    }
    fn take_fault(&self, chat: i64, method: &str) -> Option<String> {
        let mut m = self.faults.lock().unwrap();
        let q = m.get_mut(&(chat, method.to_string()))?;
        if q.is_empty() {
            return None;
        }
        Some(q.remove(0))
    }
    fn next_message_id(&self) -> i64 {
        let mut n = self.next_id.lock().unwrap();
        *n += 1;
        1000 + *n
    }
}

impl Call {
    /// Chat the call targeted (0 when the method carries none).
    pub fn chat_id(&self) -> i64 {
        self.body["chat_id"].as_i64().unwrap_or(0)
    }
}

/// One fake for the whole test binary, hosted on its OWN thread: a
/// `#[tokio::test]` runtime dies with its test, which would take the
/// accept loop (and every later test's endpoint) down with it.
static GLOBAL: std::sync::OnceLock<(Arc<FakeTg>, String)> = std::sync::OnceLock::new();

/// Start (once) and return the shared log + base URL
/// (`http://127.0.0.1:PORT`). Synchronous by design: the server thread
/// outlives the caller's runtime.
pub fn start() -> (Arc<FakeTg>, String) {
    GLOBAL
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("fake tg runtime");
                rt.block_on(async {
                    let listener = TcpListener::bind("127.0.0.1:0")
                        .await
                        .expect("fake tg bind");
                    let url = format!("http://{}", listener.local_addr().expect("fake tg addr"));
                    let fake = Arc::new(FakeTg::default());
                    tx.send((fake.clone(), url.clone())).ok();
                    loop {
                        let Ok((mut sock, _)) = listener.accept().await else {
                            continue;
                        };
                        let f = fake.clone();
                        tokio::spawn(async move {
                            let _ = serve_one(&mut sock, f).await;
                        });
                    }
                });
            });
            rx.recv().expect("fake tg boot")
        })
        .clone()
}

async fn serve_one(sock: &mut tokio::net::TcpStream, fake: Arc<FakeTg>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    // Head: until the blank line. Body: Content-Length bytes.
    let head_end = loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = find_head_end(&buf) {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + 4 + len {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end + 4..]).to_string();
    let body: Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
    // Method is the last path segment: /bot<token>/<method>
    let first = head.lines().next().unwrap_or("");
    let method = first
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let chat = body["chat_id"].as_i64().unwrap_or(0);
    let assigned = fake.next_message_id();
    fake.calls.lock().unwrap().push(Call {
        method: method.clone(),
        body,
        assigned_id: None,
    });

    let payload = match fake.take_fault(chat, &method) {
        Some(desc) => json!({"ok": false, "error_code": 400, "description": desc}),
        None => match method.as_str() {
            "sendMessage" => {
                // Record the id we hand out so tests can reference the
                // message the owner would see.
                if let Some(c) = fake.calls.lock().unwrap().last_mut() {
                    c.assigned_id = Some(assigned);
                }
                json!({"ok": true, "result": {"message_id": assigned, "date": 1}})
            }
            "getMe" => json!({"ok": true, "result": {
                "id": 42, "is_bot": true, "username": "herdr_test_bot",
                "first_name": "herdr"
            }}),
            "getUpdates" => json!({"ok": true, "result": []}),
            "createForumTopic" => json!({"ok": true, "result": {
                "message_thread_id": 900, "name": "t"
            }}),
            _ => json!({"ok": true, "result": true}),
        },
    };
    let text = payload.to_string();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        text.len(),
        text
    );
    sock.write_all(resp.as_bytes()).await?;
    sock.flush().await
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
