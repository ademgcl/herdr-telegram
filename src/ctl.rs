//! Local control server & CLI client on the single-instance guard port (47319).
//! Enables zero-split-brain inspection, single-topic reset, and event mocking.
use crate::{
    handlers::reset::run_single_topic_reset,
    herdr::{
        client::{get_agent, list_panes, list_workspaces},
        labels::pane_facts,
    },
    notifier::status::observe_status,
    state::AppState,
    types::Res,
    ui::ws_label,
};
use std::collections::HashSet;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub async fn run_control_server(s: AppState, listener: TcpListener) {
    loop {
        let (socket, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => break,
        };
        let s2 = s.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut lines = BufReader::new(reader).lines();
            if let Ok(Some(line)) = lines.next_line().await {
                let resp = handle_cmd(&s2, line.trim()).await;
                let _ = writer.write_all(resp.as_bytes()).await;
                let _ = writer.flush().await;
            }
        });
    }
}

async fn handle_cmd(s: &AppState, line: &str) -> String {
    let (cmd, args) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c.trim(), a.trim()),
        None => (line.trim(), ""),
    };

    match cmd {
        "ping" => "PONG\n".to_string(),
        "status" => {
            let topics_cnt = s.topics.all_mappings().len();
            format!("OK: herdr-telegram running, {topics_cnt} topic(s) mapped\n")
        }
        "topics" => report_topics(s).await,
        "reset" => {
            if args.is_empty() {
                "ERR: usage: reset <pane_or_topic_id>\n".to_string()
            } else {
                let forum = s.cfg.forum.unwrap_or(0);
                match run_single_topic_reset(s, forum, None, args).await {
                    Ok(msg) => format!("OK: {msg}\n"),
                    Err(e) => format!("ERR: {e}\n"),
                }
            }
        }
        "trigger" => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() < 2 {
                "ERR: usage: trigger <pane> <status> (e.g. trigger w1:p2 blocked)\n".to_string()
            } else {
                let (pane, status) = (parts[0], parts[1]);
                observe_status(s, pane, status, false, "ctl").await;
                format!("OK: triggered '{status}' on pane '{pane}'\n")
            }
        }
        "inspect" => {
            if args.is_empty() {
                "ERR: usage: inspect <pane>\n".to_string()
            } else {
                inspect_pane(s, args).await
            }
        }
        _ => "ERR: unknown command. Supported: ping, status, topics, reset, trigger, inspect\n"
            .to_string(),
    }
}

async fn report_topics(s: &AppState) -> String {
    let mappings = s.topics.all_mappings();
    let live_panes = list_panes(&s.cfg.socket).await.unwrap_or_default();
    let live_set: HashSet<String> = live_panes.iter().cloned().collect();
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let facts = pane_facts(&s.cfg.socket).await.unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!(
        "{:<10} {:<10} {:<10} {:<32}\n",
        "PANE", "THREAD", "STATE", "TITLE / WS"
    ));
    out.push_str(&format!("{}\n", "-".repeat(66)));

    // Active & Zombie topics
    for (pane, th) in &mappings {
        let is_live = live_set.contains(pane);
        let state_str = if is_live { "LIVE" } else { "ZOMBIE" };
        let title = s
            .topics
            .storage
            .get_title(pane)
            .or_else(|| facts.get(pane).and_then(|f| f.label.clone()))
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "{:<10} #{:<9} {:<10} {:<32}\n",
            pane, th, state_str, title
        ));
    }

    // Missing topics (live panes with no mapped topic)
    let mapped_set: HashSet<String> = mappings.into_keys().collect();
    for pane in &live_panes {
        if !mapped_set.contains(pane) {
            let ws = facts.get(pane).map(|f| f.ws.as_str()).unwrap_or("");
            let space = ws_label(&spaces, ws);
            out.push_str(&format!(
                "{:<10} {:<10} {:<10} [missing in space: {}]\n",
                pane, "-", "MISSING", space
            ));
        }
    }
    out
}

async fn inspect_pane(s: &AppState, pane: &str) -> String {
    let facts = pane_facts(&s.cfg.socket).await.unwrap_or_default();
    let pf = facts.get(pane);
    let th = s.topics.storage.get_thread(pane);
    let title = s.topics.storage.get_title(pane);
    let tag = s.topics.storage.get_tag(pane);
    let recent = s.topics.get_recent_msgs(pane);
    let agent = get_agent(&s.cfg.socket, pane).await.ok();

    format!(
        "=== PANE INSPECTION: {pane} ===\n\
         Thread ID:   {}\n\
         Topic Title: {}\n\
         Tag:         {}\n\
         Herdr Label: {}\n\
         Herdr Space: {}\n\
         Agent Kind:  {}\n\
         Status:      {}\n\
         Recent Msgs: {:?}\n\
         ==============================\n",
        th.map(|t| format!("#{t}")).unwrap_or_else(|| "none".into()),
        title.as_deref().unwrap_or("-"),
        tag.as_deref().unwrap_or("-"),
        pf.and_then(|f| f.label.as_deref()).unwrap_or("-"),
        pf.map(|f| f.ws.as_str()).unwrap_or("-"),
        agent.as_ref().map(|a| a.kind.as_str()).unwrap_or("shell"),
        agent.as_ref().map(|a| a.status.as_str()).unwrap_or("ready"),
        recent
    )
}

pub async fn run_ctl_client(port: u16, args: &[String]) -> Res<()> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("⚠️  Cannot connect to herdr-telegram on {addr}: {e}");
            eprintln!("   Is the bot running? Start it with: ./dev.sh start");
            return Err(e.into());
        }
    };

    let cmd_line = format!("{}\n", args.join(" "));
    stream.write_all(cmd_line.as_bytes()).await?;
    stream.flush().await?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        println!("{line}");
    }
    Ok(())
}
