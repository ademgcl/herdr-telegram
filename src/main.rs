use std::{
    collections::HashMap,
    env,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{Mutex, Notify},
};

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const TG_POLL_SECS: i64 = 25;
const STALE_SECS: u64 = 600;
const PROMPT_TIMEOUT_MS: u64 = 300_000;
const MAX_MSG_UNITS: usize = 3900;

struct Cfg {
    token: String,
    socket: String,
    owners: Vec<i64>,
}

struct State {
    cfg: Cfg,
    http: reqwest::Client,
    offset: Mutex<u64>,
    status: Mutex<HashMap<String, String>>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    // reply-routing: (chat, bot message id) -> agent pane; plus last-interacted pane
    targets: Mutex<HashMap<(i64, i64), String>>,
    torder: Mutex<std::collections::VecDeque<(i64, i64)>>,
    focus: Mutex<Option<String>>,
    // chat -> pane awaiting raw keys as next message (⌨️ button)
    keywait: Mutex<HashMap<i64, String>>,
}

fn load_env_file() {
    let Ok(txt) = std::fs::read_to_string(".env") else { return };
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        let k = k.trim();
        let mut v = v.trim().to_string();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = v[1..v.len() - 1].to_string();
        }
        if env::var_os(k).is_none() {
            // SAFETY: single-threaded init before tokio runtime starts
            unsafe { env::set_var(k, v) };
        }
    }
}

fn cfg_from_env() -> Res<Cfg> {
    load_env_file();
    let token = env::var("TELEGRAM_BOT_TOKEN").map_err(|_| "TELEGRAM_BOT_TOKEN not set")?;
    let mut owners = Vec::new();
    for (k, v) in env::vars() {
        if (k == "TELEGRAM_CHAT_ID" || k.starts_with("TELEGRAM_CHAT_ID_"))
            && let Ok(id) = v.trim().parse::<i64>()
        {
            owners.push(id);
        }
    }
    // .env often carries the same id under several keys (TELEGRAM_CHAT_ID +
    // TELEGRAM_CHAT_ID_<NAME>) — duplicates would double every broadcast
    owners.sort_unstable();
    owners.dedup();
    if owners.is_empty() {
        return Err("no TELEGRAM_CHAT_ID* owner ids found".into());
    }
    let home = env::var("HOME").unwrap_or_default();
    let socket = env::var("HERDR_SOCKET")
        .unwrap_or_else(|_| format!("{home}/.config/herdr/herdr.sock"));
    Ok(Cfg { token, socket, owners })
}

// ---------- herdr client: one exchange per connection ----------

async fn rpc(s: &State, method: &str, params: Value) -> Res<Value> {
    rpc_t(s, method, params, 30).await
}

/// long waits (agent.prompt) need their own budget — a fixed 30s cap
/// silently kills any prompt whose agent works longer than that.
async fn rpc_t(s: &State, method: &str, params: Value, timeout_secs: u64) -> Res<Value> {
    let fut = async {
        let mut conn = UnixStream::connect(&s.cfg.socket).await?;
        let req = json!({"id": "tg", "method": method, "params": params});
        conn.write_all(format!("{req}\n").as_bytes()).await?;
        conn.flush().await?;
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let v: Value = serde_json::from_str(line.trim())?;
        if let Some(e) = v.get("error") {
            return Err(e["message"].as_str().unwrap_or("herdr error").to_string().into());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    };
    tokio::time::timeout(Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| format!("herdr {method} timed out"))?
}

#[derive(Clone)]
struct AgentRow {
    kind: String,
    pane: String,
    title: String,
    status: String,
    ws: String,
}

async fn list_agents(s: &State) -> Res<Vec<AgentRow>> {
    let r = rpc(s, "agent.list", json!({})).await?;
    let mut out = Vec::new();
    if let Some(arr) = r["agents"].as_array() {
        for a in arr {
            out.push(AgentRow {
                kind: a["agent"].as_str().unwrap_or("?").into(),
                pane: a["pane_id"].as_str().unwrap_or("?").into(),
                title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
                status: a["agent_status"].as_str().unwrap_or("unknown").into(),
                ws: a["workspace_id"].as_str().unwrap_or("?").into(),
            });
        }
    }
    Ok(out)
}

async fn read_output(s: &State, target: &str, lines: u32) -> Res<String> {
    let r = rpc(
        s,
        "agent.read",
        json!({"target": target, "source": "recent_unwrapped", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

// ---------- telegram client ----------

async fn tg_call(s: &State, method: &str, body: Value, timeout: Duration) -> Res<Value> {
    let url = format!("https://api.telegram.org/bot{}/{}", s.cfg.token, method);
    let resp = s.http.post(&url).json(&body).timeout(timeout).send().await?;
    let v: Value = resp.json().await?;
    if v["ok"].as_bool() != Some(true) {
        return Err(
            v["description"].as_str().unwrap_or("telegram error").to_string().into(),
        );
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

async fn get_updates(s: &State, offset: u64, poll_secs: i64) -> Res<Vec<Value>> {
    let r = tg_call(
        s,
        "getUpdates",
        json!({
            "offset": offset,
            "timeout": poll_secs,
            "limit": 100,
            "allowed_updates": ["message", "callback_query"],
        }),
        Duration::from_secs(poll_secs as u64 + 10),
    )
    .await?;
    Ok(r.as_array().cloned().unwrap_or_default())
}

fn fit(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_string();
    }
    let marker = "\n\n… [truncated] …\n\n";
    let head = max_units * 60 / 100;
    let tail = max_units - head - marker.encode_utf16().count();
    let mut h = String::new();
    for ch in text.chars() {
        if h.encode_utf16().count() + ch.len_utf16() > head {
            break;
        }
        h.push(ch);
    }
    let rest = &text[text.char_indices().nth(h.chars().count()).map(|(i, _)| i).unwrap_or(0)..];
    let mut t = String::new();
    for ch in rest.chars().rev() {
        if t.encode_utf16().count() + ch.len_utf16() > tail {
            break;
        }
        t.insert(0, ch);
    }
    format!("{h}{marker}{t}")
}

async fn send(s: &State, chat_id: i64, text: &str) -> Option<i64> {
    send_kb(s, chat_id, text, None).await
}

async fn send_kb(s: &State, chat_id: i64, text: &str, keyboard: Option<Value>) -> Option<i64> {
    let body = fit(text, MAX_MSG_UNITS);
    let mut params = json!({"chat_id": chat_id, "text": body});
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    for attempt in 0..2 {
        match tg_call(s, "sendMessage", params.clone(), Duration::from_secs(15)).await {
            Ok(v) => return v["message_id"].as_i64(),
            Err(e) => {
                eprintln!("send failed (attempt {}): {e}", attempt + 1);
                // a timed-out request may still have been delivered — retrying
                // would produce an exact duplicate message. only retry when we
                // are sure the request never reached Telegram.
                let retryable = e
                    .downcast_ref::<reqwest::Error>()
                    .map(|re| re.is_connect())
                    .unwrap_or(false);
                if !retryable {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    None
}

/// remember which agent a bot message belongs to, so replying to it targets that agent
async fn remember(s: &State, chat: i64, msg_id: Option<i64>, pane: &str) {
    let Some(msg_id) = msg_id else { return };
    let mut ord = s.torder.lock().await;
    let mut map = s.targets.lock().await;
    while map.len() >= 512 {
        match ord.pop_front() {
            Some(old) => {
                map.remove(&old);
            }
            None => break,
        }
    }
    if map.insert((chat, msg_id), pane.to_string()).is_none() {
        ord.push_back((chat, msg_id));
    }
}

async fn set_focus(s: &State, pane: &str) {
    *s.focus.lock().await = Some(pane.to_string());
}

fn emoji(status: &str) -> &'static str {
    match status {
        "working" => "🔄",
        "idle" => "🟢",
        "blocked" => "⛔",
        "done" => "✅",
        _ => "❔",
    }
}

// ---------- herdr-style navigator (spaces → workspace → agent) ----------

fn btn(text: String, data: &str) -> Value {
    json!({"text": text, "callback_data": data})
}

fn worst_status(rows: &[&AgentRow]) -> &'static str {
    if rows.iter().any(|r| r.status == "blocked") {
        "⛔"
    } else if rows.iter().any(|r| r.status == "working") {
        "🔄"
    } else if rows.iter().any(|r| r.status == "done") {
        "✅"
    } else if rows.iter().any(|r| r.status == "idle") {
        "🟢"
    } else {
        "▫️"
    }
}

async fn list_workspaces(s: &State) -> Res<Vec<(String, String, u64)>> {
    let r = rpc(s, "workspace.list", json!({})).await?;
    let mut out = Vec::new();
    for w in r["workspaces"].as_array().cloned().unwrap_or_default() {
        out.push((
            w["workspace_id"].as_str().unwrap_or("?").to_string(),
            w["label"].as_str().unwrap_or("?").to_string(),
            w["number"].as_u64().unwrap_or(0),
        ));
    }
    Ok(out)
}

async fn build_menu(s: &State) -> Res<(String, Value)> {
    let spaces = list_workspaces(s).await?;
    let agents = list_agents(s).await?;
    let label = |id: &str| {
        spaces
            .iter()
            .find(|(sid, _, _)| sid == id)
            .map(|(_, l, _)| l.clone())
            .unwrap_or_else(|| id.to_string())
    };

    // ---- spaces section ----
    let mut text = String::from("🗂 spaces\n");
    let mut kb: Vec<Vec<Value>> = Vec::new();
    for (id, sp_label, num) in &spaces {
        let mine: Vec<&AgentRow> = agents.iter().filter(|a| &a.ws == id).collect();
        let emo = worst_status(&mine);
        text.push_str(&format!("{emo} #{num} {sp_label} — {} agent(s)\n", mine.len()));
    }
    for pair in spaces.chunks(2) {
        kb.push(
            pair.iter()
                .map(|(id, l, _)| {
                    let mine: Vec<&AgentRow> = agents.iter().filter(|a| &a.ws == id).collect();
                    btn(format!("{} {}", worst_status(&mine), l), &format!("w:{id}"))
                })
                .collect(),
        );
    }
    kb.push(vec![btn("➕ spawn agent".into(), "n")]);

    // ---- agents section ----
    text.push_str("\n🤖 agents\n");
    if agents.is_empty() {
        text.push_str("(none — tap ➕ or wait for detection)\n");
    }
    for a in &agents {
        text.push_str(&format!(
            "{} {} @ {} [{}]\n",
            emoji(&a.status),
            a.kind,
            label(&a.ws),
            a.pane,
        ));
        kb.push(vec![btn(
            format!("{} {} @ {}", emoji(&a.status), a.kind, label(&a.ws)),
            &format!("a:{}", a.pane),
        )]);
    }
    Ok((text, json!(kb)))
}

const SPAWN_KINDS: &[&str] = &[
    "opencode", "claude", "codex", "gemini", "cursor", "copilot", "amp", "droid", "grok", "qwen",
];

fn spawn_kb() -> Value {
    let mut rows: Vec<Vec<Value>> = SPAWN_KINDS
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|k| btn((*k).to_string(), &format!("k:{k}")))
                .collect()
        })
        .collect();
    rows.push(vec![btn("← back".into(), "m")]);
    json!(rows)
}

/// ensure a dedicated "tg" space exists so remote spawns never disturb
/// the user's local layout; returns its workspace_id
async fn ensure_tg_space(s: &State) -> Res<String> {
    for (id, label, _) in list_workspaces(s).await? {
        if label == "tg" {
            return Ok(id);
        }
    }
    let r = rpc_t(s, "workspace.create", json!({"label": "tg"}), 30).await?;
    Ok(r["workspace"]["workspace_id"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

async fn spawn_agent(s: &State, kind: &str) -> Res<AgentRow> {
    let ws = ensure_tg_space(s).await?;
    // fresh tab per spawn => fresh root pane, no layout math
    let tab = rpc_t(s, "tab.create", json!({"workspace_id": ws}), 30).await?;
    let pane = tab["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // names must be unique among live agents; kind list has no names to check
    let name = format!("tg-{kind}-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs());

    rpc_t(
        s,
        "agent.start",
        json!({"name": name, "kind": kind, "pane_id": pane}),
        90, // herdr waits for the agent to become ready
    )
    .await?;

    let v = rpc(s, "agent.get", json!({"target": pane})).await?;
    let a = &v["agent"];
    Ok(AgentRow {
        kind: a["agent"].as_str().unwrap_or(kind).into(),
        pane: a["pane_id"].as_str().unwrap_or(&pane).into(),
        title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
        status: a["agent_status"].as_str().unwrap_or("unknown").into(),
        ws: a["workspace_id"].as_str().unwrap_or(&ws).into(),
    })
}

async fn build_ws_view(s: &State, ws: &str) -> Res<(String, Value)> {
    let spaces = list_workspaces(s).await?;
    let agents: Vec<AgentRow> = list_agents(s)
        .await?
        .into_iter()
        .filter(|a| a.ws == ws)
        .collect();
    let label = spaces
        .iter()
        .find(|(id, _, _)| id == ws)
        .map(|(_, l, n)| format!("#{n} {l}"))
        .unwrap_or_else(|| ws.to_string());

    let mut text = format!("🖥 {label}\n\n");
    let mut kb = Vec::new();
    if agents.is_empty() {
        text.push_str("(no live agents here)");
    }
    for a in &agents {
        let title: String = a.title.chars().take(36).collect();
        text.push_str(&format!("{} {} [{}]\n   {title}\n", emoji(&a.status), a.kind, a.pane));
        kb.push(vec![btn(
            format!("{} {} · {title}", emoji(&a.status), a.kind),
            &format!("a:{}", a.pane),
        )]);
    }
    kb.push(vec![btn("← spaces".into(), "m")]);
    Ok((text, json!(kb)))
}

async fn build_agent_card(s: &State, pane: &str) -> Res<(String, Value)> {
    let v = rpc(s, "agent.get", json!({"target": pane})).await?;
    let a = &v["agent"];
    let status = a["agent_status"].as_str().unwrap_or("unknown");
    let ws = a["workspace_id"].as_str().unwrap_or("?");
    let kind = a["agent"].as_str().unwrap_or("?");
    let title = a["terminal_title_stripped"].as_str().unwrap_or("");
    let cwd = a["foreground_cwd"]
        .as_str()
        .or(a["cwd"].as_str())
        .unwrap_or("");
    let text = format!(
        "{} {} [{}]\nstatus: {status}\nspace: {ws}\ncwd: {cwd}\ntitle: {title}",
        emoji(status),
        kind,
        pane,
    );
    let kb = json!([
        [
            btn("📄 output".into(), &format!("o:{pane}")),
            btn("⌨️ keys".into(), &format!("K:{pane}")),
        ],
        [
            btn("🔄 refresh".into(), &format!("a:{pane}")),
            btn(format!("← {}", ws), &format!("w:{ws}")),
        ],
    ]);
    Ok((text, kb))
}

async fn handle_callback(s: Arc<State>, cbq: &Value) {
    let Some(from) = cbq["from"]["id"].as_i64() else { return };
    if !s.cfg.owners.contains(&from) {
        return; // silently ignore strangers
    }
    let chat = cbq["message"]["chat"]["id"].as_i64();
    let msg_id = cbq["message"]["message_id"].as_i64();
    let data = cbq["data"].as_str().unwrap_or("");

    // clear the loading spinner right away, no popup
    let _ = tg_call(
        &s,
        "answerCallbackQuery",
        json!({"callback_query_id": cbq["id"]}),
        Duration::from_secs(10),
    )
    .await;
    let (Some(chat), Some(msg_id)) = (chat, msg_id) else { return };

    let route: Vec<&str> = data.splitn(2, ':').collect();
    if route.as_slice() == ["n"] {
        edit_kb(&s, chat, msg_id, "spawn which agent?", Some(spawn_kb())).await;
        return;
    }
    if let ["k", kind] = route.as_slice() {
        edit_kb(&s, chat, msg_id, &format!("⏳ starting {kind}…"), None).await;
        match spawn_agent(&s, kind).await {
            Ok(row) => {
                remember(&s, chat, Some(msg_id), &row.pane).await;
                set_focus(&s, &row.pane).await;
                match build_agent_card(&s, &row.pane).await {
                    Ok((text, kb)) => edit_kb(&s, chat, msg_id, &text, Some(kb)).await,
                    Err(e) => edit_kb(&s, chat, msg_id, &format!("✅ started\n⚠️ {e}"), None).await,
                }
            }
            Err(e) => edit_kb(&s, chat, msg_id, &format!("⚠️ spawn failed: {e}"), None).await,
        }
        return;
    }
    if let ["K", pane] = route.as_slice() {
        // arm keys-mode: next plain text is sent as raw keys
        s.keywait.lock().await.insert(chat, pane.to_string());
        set_focus(&s, pane).await;
        edit_kb(
            &s,
            chat,
            msg_id,
            &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"),
            None,
        )
        .await;
        return;
    }

    let view: Option<Res<(String, Value)>> = match route.as_slice() {
        ["m"] => Some(build_menu(&s).await),
        ["w", ws] => Some(build_ws_view(&s, ws).await),
        ["a", pane] => {
            // the card message itself becomes reply-addressable to this agent
            remember(&s, chat, Some(msg_id), pane).await;
            set_focus(&s, pane).await;
            Some(build_agent_card(&s, pane).await)
        }
        ["o", pane] => {
            // long-form content: new message instead of in-place edit
            let out = read_output(&s, pane, 120).await.unwrap_or_default();
            let body = if out.is_empty() { "(no output)".to_string() } else { out };
            let mid = send_kb(
                &s,
                chat,
                &body,
                Some(json!([[btn("← back".into(), &format!("a:{pane}"))]])),
            )
            .await;
            remember(&s, chat, mid, pane).await;
            set_focus(&s, pane).await;
            None
        }
        _ => None,
    };

    if let Some(result) = view {
        match result {
            Ok((text, kb)) => edit_kb(&s, chat, msg_id, &text, Some(kb)).await,
            Err(e) => edit_kb(&s, chat, msg_id, &format!("⚠️ {e}"), None).await,
        }
    }
}

async fn edit_kb(s: &State, chat_id: i64, message_id: i64, text: &str, keyboard: Option<Value>) {
    let body = fit(text, MAX_MSG_UNITS);
    let mut params = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": body,
    });
    if let Some(kb) = keyboard {
        params["reply_markup"] = json!({"inline_keyboard": kb});
    }
    if let Err(e) = tg_call(s, "editMessageText", params, Duration::from_secs(15)).await {
        let msg = e.to_string();
        if !msg.contains("message is not modified") {
            eprintln!("edit failed: {msg}");
        }
    }
}

// ---------- status transitions & notifications ----------

async fn observe_status(s: &State, pane: &str, new_status: &str, silent: bool, src: &str) {
    let old = {
        let mut m = s.status.lock().await;
        m.insert(pane.to_string(), new_status.to_string())
    };
    if silent || old.as_deref() == Some(new_status) {
        return;
    }
    let is_attention = matches!(new_status, "blocked" | "done" | "idle");
    if !is_attention {
        return;
    }
    // herdr flips working→done→idle within moments when the local TUI has
    // seen the tab — done and idle mean the same thing here, so collapse
    // that flip-flop into one alert
    if old.as_deref() == Some("done") && new_status == "idle"
        || old.as_deref() == Some("idle") && new_status == "done"
    {
        println!("[alert] collapsed {old:?}→{new_status} for {pane} ({src})");
        return;
    }
    // an active prompt job reports its own outcome — suppress the parallel alert
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }
    println!("[alert] {src}: {pane} {old:?}→{new_status}");
    let (kind, ws_id, title) = match rpc(s, "agent.get", json!({"target": pane})).await {
        Ok(v) => {
            let a = &v["agent"];
            (
                a["agent"].as_str().unwrap_or("?").to_string(),
                a["workspace_id"].as_str().unwrap_or("?").to_string(),
                a["terminal_title_stripped"].as_str().unwrap_or("").to_string(),
            )
        }
        Err(_) => ("?".into(), "?".into(), String::new()),
    };
    let space = list_workspaces(s)
        .await
        .ok()
        .and_then(|ws| ws.into_iter().find(|(id, _, _)| *id == ws_id))
        .map(|(_, label, num)| format!("#{num} {label}"))
        .unwrap_or_else(|| ws_id.clone());
    let hint = match new_status {
        "blocked" => "\n↩️ reply to answer",
        _ => "",
    };
    let verb = if new_status == "idle" { "ready" } else { new_status };
    let mut text = format!("{} {}: {kind} @ {space}", emoji(new_status), verb);
    if !title.is_empty() {
        let short: String = title.chars().take(60).collect();
        text.push_str(&format!("\n{short}"));
    }
    text.push_str(hint);
    set_focus(s, pane).await;
    for id in &s.cfg.owners {
        let mid = send_kb(
            s,
            *id,
            &text,
            Some(json!([[btn("show output".into(), &format!("o:{pane}"))]])),
        )
        .await;
        remember(s, *id, mid, pane).await;
    }
}

async fn reconcile(s: &State, silent: bool, src: &str) {
    if let Ok(rows) = list_agents(s).await {
        for r in rows {
            observe_status(s, &r.pane, &r.status, silent, src).await;
        }
    }
}

// ---------- herdr event stream task ----------

async fn event_task(s: Arc<State>) {
    loop {
        match run_stream(&s).await {
            Ok(reason) => println!("[events] resubscribe ({reason})"),
            Err(e) => eprintln!("[events] stream error: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        reconcile(&s, false, "reconnect").await; // catch up on anything missed while disconnected
    }
}

async fn run_stream(s: &Arc<State>) -> Res<&'static str> {
    let agents = list_agents(s).await.map_err(|e| e.to_string())?;
    // per-pane subscriptions only — herdr requires pane_id and has no wildcard;
    // panes created later are covered by the periodic watchdog reconcile.
    let subs: Vec<Value> = agents
        .iter()
        .map(|a| json!({"type": "pane.agent_status_changed", "pane_id": a.pane}))
        .collect();
    if subs.is_empty() {
        // nothing to watch yet; idle out so the outer loop retries on schedule
        tokio::time::sleep(Duration::from_secs(20)).await;
        return Ok("no agents to watch");
    }

    let conn = UnixStream::connect(&s.cfg.socket).await?;
    let (reader, mut writer) = conn.into_split();
    writer
        .write_all(
            json!({"id": "sub", "method": "events.subscribe",
                   "params": {"subscriptions": subs}})
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
        if ev["event"].as_str() == Some("pane_agent_status_changed")
            && let Some(pane) = ev["data"]["pane_id"].as_str()
        {
            // authoritative status via agent.get; event payload may be partial
            let status = match rpc(s, "agent.get", json!({"target": pane})).await {
                Ok(v) => v["agent"]["agent_status"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                Err(e) => {
                    eprintln!("[events] agent.get failed: {e}");
                    continue;
                }
            };
            observe_status(s, pane, &status, false, "event").await;
        }
    }
}

// ---------- prompt jobs ----------

struct Job {
    cancel: Notify,
    stopped: std::sync::atomic::AtomicBool,
    queue: Mutex<std::collections::VecDeque<(i64, String)>>,
}

/// one line per live agent; ▶️ marks who just received your message
async fn agents_summary(s: &State, highlight: &str) -> String {
    let spaces = list_workspaces(s).await.unwrap_or_default();
    let label = |id: &str| {
        spaces
            .iter()
            .find(|(sid, _, _)| sid == id)
            .map(|(_, l, _)| l.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let rows = list_agents(s).await.unwrap_or_default();
    let mut out = String::new();
    for r in &rows {
        let mark = if r.pane == highlight { "▶️" } else { "·" };
        out.push_str(&format!(
            "{mark}{} {} @ {}\n",
            emoji(&r.status),
            r.kind,
            label(&a_ws(r)),
        ));
    }
    out
}

fn a_ws(r: &AgentRow) -> String {
    r.ws.clone()
}

async fn enqueue_prompt(s: Arc<State>, chat: i64, row: AgentRow, text: String) {
    let existing = s.jobs.lock().await.get(&row.pane).cloned();
    if let Some(job) = existing {
        job.queue.lock().await.push_back((chat, text));
        let summary = agents_summary(&s, &row.pane).await;
        let mid = send(
            &s,
            chat,
            &format!("📨 queued — runs after the current task\n\n{summary}"),
        )
        .await;
        remember(&s, chat, mid, &row.pane).await;
        return;
    }
    let job = Arc::new(Job {
        cancel: Notify::new(),
        stopped: std::sync::atomic::AtomicBool::new(false),
        queue: Mutex::new(Default::default()),
    });
    s.jobs.lock().await.insert(row.pane.clone(), job.clone());
    tokio::spawn(run_job(s.clone(), chat, row, text, job));
}

async fn run_job(s: Arc<State>, chat: i64, row: AgentRow, first: String, job: Arc<Job>) {
    let pane = row.pane.clone();
    let mut current = Some((chat, first));
    loop {
        let item = match current.take() {
            Some(x) => x,
            None => match job.queue.lock().await.pop_front() {
                Some(x) => x,
                None => break,
            },
        };
        let (to_chat, text) = item;

        let summary = agents_summary(&s, &pane).await;
        let mid = send(&s, to_chat, &format!("📨 got it — {0} is on it\n\n{summary}", row.kind))
            .await;
        remember(&s, to_chat, mid, &pane).await;
        set_focus(&s, &pane).await;

        process_prompt(&s, to_chat, &pane, text, &job).await;
        if job.stopped.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
    }
    println!("[prompt] job done: {pane}");
    s.jobs.lock().await.remove(&pane);
}

async fn process_prompt(s: &Arc<State>, chat: i64, pane: &str, text: String, job: &Job) {
    println!("[prompt] -> {pane}: {}", text.chars().take(80).collect::<String>());
    let fut = rpc_t(
        s,
        "agent.prompt",
        json!({
            "target": pane,
            "text": text,
            "wait": {"until": ["idle", "done", "blocked"], "timeout_ms": PROMPT_TIMEOUT_MS}
        }),
        PROMPT_TIMEOUT_MS / 1000 + 60,
    );

    tokio::select! {
        _ = job.cancel.notified() => {
            job.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
            let mid = send(s, chat, "✋ cancelled").await;
            remember(s, chat, mid, pane).await;
        }
        r = fut => {
            // a wait-timeout is NOT a failure: the agent keeps working.
            // fall back to open-ended watch rounds until it really settles.
            let wait_timed_out = match &r {
                Err(e) => e.to_string().contains("agent.prompt timed out"),
                Ok(_) => false,
            };
            let mut msg = if let Err(e) = &r {
                if wait_timed_out {
                    String::new()
                } else {
                    format!("⚠️ error: {e}")
                }
            } else {
                String::new()
            };

            if wait_timed_out {
                send(s, chat, "⏳ still working — I'll ping you when it finishes").await;
                for round in 0..12 {
                    if job.stopped.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let w = rpc_t(
                        s,
                        "agent.wait",
                        json!({
                            "target": pane,
                            "until": ["idle", "done", "blocked"],
                            "timeout_ms": 600_000
                        }),
                        660,
                    )
                    .await;
                    let settled = match &w {
                        Ok(v) => v["agent"]["agent_status"].as_str()
                            .or(v["agent_status"].as_str())
                            .map(|x| x.to_string()),
                        Err(e) => {
                            if e.to_string().contains("agent.wait timed out") {
                                if round % 3 == 2 {
                                    send(s, chat, "⏳ still going…").await;
                                }
                                continue;
                            }
                            Some("unknown".to_string())
                        }
                    };
                    let Some(settled) = settled else { continue };
                    let out = read_output(s, pane, 50).await.unwrap_or_default();
                    msg = if out.is_empty() {
                        format!("{} {settled}\n(no readable output)", emoji(&settled))
                    } else {
                        format!("{} {settled}\n\n{out}", emoji(&settled))
                    };
                    observe_status(s, pane, &settled, true, "job").await;
                    break;
                }
                if msg.is_empty() {
                    msg = "⚠️ gave up watching (agent never settled)".into();
                }
            } else if let Ok(v) = &r {
                let settled = v["agent"]["agent_status"].as_str()
                    .or(v["agent_status"].as_str())
                    .unwrap_or("unknown");
                let out = read_output(s, pane, 50).await.unwrap_or_default();
                msg = if out.is_empty() {
                    format!("{} {settled}\n(no readable output)", emoji(settled))
                } else {
                    format!("{} {settled}\n\n{out}", emoji(settled))
                };
                observe_status(s, pane, settled, true, "job").await;
            }

            let mid = send(s, chat, &msg).await;
            remember(s, chat, mid, pane).await;
            set_focus(s, pane).await;
        }
    }
}

// ---------- command handling ----------

fn help_text() -> &'static str {
    "/agents   control panel: spaces, agents, ➕ spawn\n\
     /read     recent output of focused agent\n\
     /cancel   abort prompts / exit keys-mode\n\
     /keys <pane> y enter   send raw keys\n\n\
     ↩️ reply to any bot message → talks to that agent\n\
     plain text → focused agent\n\n\
     alerts fire on ⛔ needs-input / ✅ finish — just reply to them"
}

async fn resolve_target(rows: &[AgentRow], spec: Option<&str>) -> Option<AgentRow> {
    match spec {
        None | Some("") => {
            if rows.len() == 1 {
                rows.first().cloned()
            } else {
                None
            }
        }
        Some(t) => rows
            .iter()
            .find(|r| r.pane == t)
            .cloned()
            .or_else(|| {
                let m: Vec<_> = rows.iter().filter(|r| r.kind == t).collect();
                if m.len() == 1 {
                    m.first().map(|r| (*r).clone())
                } else {
                    None
                }
            }),
    }
}

async fn handle_message(s: Arc<State>, chat: i64, msg: &Value) {
    let text = msg["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return;
    }
    let (cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };

    // replying to any bot message about an agent targets that agent
    let reply_pane: Option<String> = match msg["reply_to_message"]["message_id"].as_i64() {
        Some(rid) => s.targets.lock().await.get(&(chat, rid)).cloned(),
        None => None,
    };

    if cmd == "/start" || cmd == "/help" {
        send(&s, chat, help_text()).await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&chat);
        let jobs: HashMap<String, Arc<Job>> = std::mem::take(&mut *s.jobs.lock().await);
        for n in jobs.values() {
            n.cancel.notify_waiters();
        }
        send(
            &s,
            chat,
            &format!("✋ cancelled {} pending job(s)", jobs.len()),
        )
        .await;
        return;
    }

    // ⌨️ keys-mode armed via button: next plain message is raw keys
    if let Some(pane) = s.keywait.lock().await.remove(&chat) {
        let key_list: Vec<&str> = text.split_whitespace().collect();
        match rpc(&s, "agent.send_keys", json!({"target": pane, "keys": key_list})).await {
            Ok(_) => {
                send(&s, chat, "⌨️ sent").await;
            }
            Err(e) => {
                send(&s, chat, &format!("⚠️ {e}")).await;
            }
        }
        return;
    }
    let rows = match list_agents(&s).await {
        Ok(r) => r,
        Err(e) => {
            send(&s, chat, &format!("⚠️ herdr unreachable: {e}")).await;
            return;
        }
    };

    if cmd == "/agents" {
        match build_menu(&s).await {
            Ok((text, kb)) => {
                send_kb(&s, chat, &text, Some(kb)).await;
            }
            Err(e) => { send(&s, chat, &format!("⚠️ {e}")).await; }
        }
        return;
    }

    // /keys with no args: target the focused/replied agent
    if cmd == "/keys" && !arg.is_empty() {
        let (t, keys) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        let pane = if keys.is_empty() { None } else { resolve_target(&rows, Some(t)).await.map(|r| r.pane) };
        let Some(pane) = pane else {
            send(&s, chat, "usage: /keys <pane-id|kind> <key> [key...]  e.g. /keys w8:p1 y enter").await;
            return;
        };
        let key_list: Vec<&str> = keys.split_whitespace().collect();
        match rpc(&s, "agent.send_keys", json!({"target": pane, "keys": key_list})).await {
            Ok(_) => { send(&s, chat, "⌨️ sent").await; }
            Err(e) => { send(&s, chat, &format!("⚠️ {e}")).await; }
        }
        return;
    }

    // /read [target] — no args: focus > sole agent
    if cmd == "/read" {
        let row = match resolve_target(&rows, Some(arg)).await {
            Some(r) => Some(r),
            None if arg.is_empty() => s.focus.lock().await.clone().and_then(|p| rows.iter().find(|r| r.pane == p).cloned()),
            _ => None,
        };
        let Some(row) = row else {
            send(&s, chat, "unknown target — see /agents").await;
            return;
        };
        match read_output(&s, &row.pane, 80).await {
            Ok(out) => {
                let body = if out.is_empty() { "(no output)".into() } else { out };
                let mid = send(&s, chat, &body).await;
                remember(&s, chat, mid, &row.pane).await;
                set_focus(&s, &row.pane).await;
            }
            Err(e) => { send(&s, chat, &format!("⚠️ {e}")).await; }
        }
        return;
    }

    if cmd.starts_with('/') {
        send(&s, chat, "unknown command — /help").await;
        return;
    }

    // ---- bare text = prompt. resolution order:
    // 1. explicit prefix ("w8:p1 do x" / "opencode do x")
    // 2. reply-to a message about an agent
    // 3. last agent you interacted with
    // 4. the sole live agent
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));

    let explicit = if rest.is_empty() {
        None
    } else {
        resolve_target(&rows, Some(head))
            .await
            .map(|r| (r, rest.to_string()))
    };

    let via_reply = reply_pane
        .as_deref()
        .and_then(|p| rows.iter().find(|r| r.pane == p))
        .cloned();

    let via_focus = s
        .focus
        .lock()
        .await
        .clone()
        .and_then(|p| rows.iter().find(|r| r.pane == p).cloned());

    let (row, prompt_text) = if let Some(pair) = explicit {
        pair
    } else if let Some(r) = via_reply {
        (r, text.to_string())
    } else if let Some(r) = via_focus {
        (r, text.to_string())
    } else if let Some(r) = resolve_target(&rows, Some("")).await {
        (r, text.to_string())
    } else {
        send(&s, chat, "who? tap an agent in /agents, or reply to its last message").await;
        return;
    };

    if prompt_text.trim().is_empty() {
        return;
    }
    set_focus(&s, &row.pane).await;
    enqueue_prompt(s.clone(), chat, row, prompt_text).await;
}

// ---------- auth & update routing ----------

async fn handle_update(s: Arc<State>, u: &Value) {
    // inline-button taps arrive as callback_query updates
    if u.get("callback_query").is_some() {
        handle_callback(s, &u["callback_query"]).await;
        return;
    }

    let msg = &u["message"];
    let from = msg["from"]["id"].as_i64();
    let chat_id = msg["chat"]["id"].as_i64();
    let chat_type = msg["chat"]["type"].as_str().unwrap_or("");
    let text = msg["text"].as_str().unwrap_or("");

    let (Some(from), Some(chat)) = (from, chat_id) else { return };
    if chat_type != "private" || chat != from || !s.cfg.owners.contains(&from) {
        return; // silently ignore strangers/groups
    }
    if text.is_empty() {
        return;
    }
    let date = msg["date"].as_u64().unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(date) > STALE_SECS {
        println!("[tg] dropping stale update");
        return;
    }
    handle_message(s, chat, msg).await;
}

// ---------- main ----------

#[tokio::main]
async fn main() -> Res<()> {
    // single-instance guard: a second copy would double every push alert
    let _guard = tokio::net::TcpListener::bind("127.0.0.1:47319")
        .await
        .map_err(|_| "another herdr-telegram instance is already running")?;
    let cfg = cfg_from_env()?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let s = Arc::new(State {
        offset: Mutex::new(0),
        status: Mutex::new(HashMap::new()),
        jobs: Mutex::new(HashMap::new()),
        targets: Mutex::new(HashMap::new()),
        torder: Mutex::new(Default::default()),
        focus: Mutex::new(None),
        keywait: Mutex::new(HashMap::new()),
        cfg,
        http,
    });

    // protocol check
    let pong = rpc(&s, "ping", json!({})).await?;
    println!(
        "[herdr] server v{}, protocol {} (bot built against protocol 20)",
        pong["version"], pong["protocol"]
    );
    if pong["protocol"].as_u64() != Some(20) {
        eprintln!("[herdr] WARNING: unexpected protocol version — commands may fail");
    }

    // register the Telegram menu button so commands are one tap, never typed
    let _ = tg_call(
        &s,
        "setMyCommands",
        json!({"commands": [
            {"command": "agents", "description": "open control panel (spaces + agents)"},
            {"command": "read", "description": "recent output of focused agent"},
            {"command": "cancel", "description": "abort pending prompts / keys-mode"},
            {"command": "keys", "description": "/keys <pane> y enter — send raw keys"},
            {"command": "help", "description": "how to drive agents from here"},
        ]}),
        Duration::from_secs(15),
    )
    .await;

    // seed status cache without notification storm
    reconcile(&s, true, "seed").await;

    // discard any update backlog (stale-command replay protection)
    let backlog = get_updates(&s, 0, 0).await?;
    let max_id = backlog.last().and_then(|u| u["update_id"].as_u64());
    if let Some(id) = max_id {
        *s.offset.lock().await = id + 1;
    }
    if !backlog.is_empty() {
        println!("[tg] discarded {} stale update(s)", backlog.len());
    }

    tokio::spawn(event_task(s.clone()));

    let mut watchdog_tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = watchdog_tick.tick() => reconcile(&s, false, "watchdog").await,
            updates = async {
                let off = *s.offset.lock().await;
                get_updates(&s, off, TG_POLL_SECS).await
            } => {
                match updates {
                    Ok(list) => {
                        for u in list {
                            let id = u["update_id"].as_u64().unwrap_or(0);
                            {
                                let mut off = s.offset.lock().await;
                                if id >= *off { *off = id + 1; }
                            }
                            handle_update(s.clone(), &u).await;
                        }
                    }
                    Err(e) => {
                        eprintln!("[tg] poll failed: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
}
