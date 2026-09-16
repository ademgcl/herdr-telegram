//! Local control server & CLI client on the single-instance guard port (47319).
//! Enables zero-split-brain inspection, single-topic reset, and event mocking.
use crate::{
    handlers::{reset::run_single_topic_reset, title_rules::pick_core},
    herdr::{
        client::{get_agent, list_panes, list_workspaces},
        labels::{pane_facts, tab_labels},
    },
    notifier::status::observe_status,
    state::AppState,
    topics::names::format_title,
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
    let tabs = tab_labels(&s.cfg.socket).await.unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!(
        "{:<10} {:<10} {:<10} {:<16} {:<32}\n",
        "PANE", "THREAD", "STATE", "TAB", "TITLE"
    ));
    out.push_str(&format!("{}\n", "-".repeat(82)));

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
        let tab = facts
            .get(pane)
            .and_then(|f| tabs.get(&f.tab_id))
            .cloned()
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "{:<10} #{:<9} {:<10} {:<16} {:<32}\n",
            pane, th, state_str, tab, title
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
    let tabs = tab_labels(&s.cfg.socket).await.unwrap_or_default();
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let pf = facts.get(pane);
    let th = s.topics.storage.get_thread(pane);
    let title = s.topics.storage.get_title(pane);
    let tag = s.topics.storage.get_tag(pane);
    let recent = s.topics.get_recent_msgs(pane);
    let agent = get_agent(&s.cfg.socket, pane).await.ok();

    let tab_id = pf.map(|f| f.tab_id.as_str()).unwrap_or("-");
    let tab_label = pf
        .and_then(|f| tabs.get(&f.tab_id))
        .map(|t| t.as_str())
        .unwrap_or("-");
    let ws_id = pf.map(|f| f.ws.as_str()).unwrap_or("-");
    let ws_name = spaces
        .iter()
        .find(|w| w.id == ws_id)
        .map(|w| format!("#{} {} ({})", w.number, w.label, w.id))
        .unwrap_or_else(|| ws_id.to_string());
    let term_title = agent
        .as_ref()
        .map(|a| a.title.as_str())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("-");
    // Desired topic title under current rules (tab else tag; terminal never).
    let kind = agent.as_ref().map(|a| a.kind.as_str()).unwrap_or("shell");
    let space = pf
        .map(|f| ws_label(&spaces, &f.ws))
        .unwrap_or("?");
    let multi = !tab_id.is_empty()
        && tab_id != "-"
        && facts.values().filter(|f| f.tab_id == tab_id).count() > 1;
    let core = pick_core(
        if tab_label == "-" { None } else { Some(tab_label) },
        tag.as_deref().unwrap_or("?"),
        multi,
    );
    let desired = format_title(space, &core, kind);
    let suffix = if kind == "shell" {
        "none".to_string()
    } else {
        format!("·{}", kind.to_lowercase())
    };

    format!(
        "=== PANE INSPECTION: {pane} ===\n\
         Thread ID:   {}\n\
         Topic Title: {} (stored)\n\
         Desired:     {desired} (tab wins, terminal→pin only)\n\
         Tag:         {}\n\
         Tab:         {tab_label} ({tab_id})\n\
         Pane Label:  {}\n\
         Workspace:   {ws_name}\n\
         Agent Kind:  {} (suffix {}; shell=none)\n\
         Term Title:  {term_title} (pinned card only)\n\
         Status:      {}\n\
         Recent Msgs: {:?}\n\
         ==============================\n",
        th.map(|t| format!("#{t}")).unwrap_or_else(|| "none".into()),
        title.as_deref().unwrap_or("-"),
        tag.as_deref().unwrap_or("-"),
        pf.and_then(|f| f.label.as_deref()).unwrap_or("-"),
        kind,
        suffix,
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
