//! Control-socket command implementation: inspection, single-topic
//! reset, and event mocking. Split from `ctl` (transport lives there).
//! Pure-dispatch branches (ping/unknown/usage) are unit-tested; success
//! paths need a live herdr socket.
use crate::{
    handlers::{reset::run_single_topic_reset, title_rules::title_core_for},
    herdr::{
        client::{get_agent, list_panes, list_workspaces},
        labels::{pane_facts, tab_labels},
    },
    notifier::status::observe_status,
    state::AppState,
    topics::names::format_title,
    ui::ws_label,
};
use std::collections::HashSet;

/// Single source for the unknown control command (impl + test).
pub(crate) const UNKNOWN_CTL: &str =
    "ERR: unknown command. Supported: ping, status, topics, reset, trigger, inspect\n";
/// Single source for the ping ack (impl + test).
pub(crate) const PONG: &str = "PONG\n";
/// Single source for control usage lines (impl + tests).
pub(crate) const USAGE_RESET: &str = "ERR: usage: reset <pane_or_topic_id>\n";
pub(crate) const USAGE_TRIGGER: &str =
    "ERR: usage: trigger <pane> <status> (e.g. trigger w1:p2 blocked)\n";
pub(crate) const USAGE_TRIGGER_STATUS: &str =
    "ERR: usage: trigger <pane> <blocked|working|done|idle|shell>\n";
pub(crate) const USAGE_INSPECT: &str = "ERR: usage: inspect <pane>\n";

/// Single source for the degraded-read banner (impl + test).
pub(crate) const DEGRADED_BANNER: &str = "⚠️ herdr read degraded — states may be stale\n";

/// Pure degraded verdict over the four herdr reads (pure, tested):
/// any Err means the table below may be stale.
pub(crate) fn degraded_banner(
    live_err: bool,
    spaces_err: bool,
    facts_err: bool,
    tabs_err: bool,
) -> Option<&'static str> {
    if live_err || spaces_err || facts_err || tabs_err {
        Some(DEGRADED_BANNER)
    } else {
        None
    }
}

pub(crate) async fn handle_cmd(s: &AppState, line: &str) -> String {
    // Trim first: TCP clients (netcat, scripts) often send leading or
    // trailing whitespace, which must not turn `reset` into "unknown".
    let line = line.trim();
    let (cmd, args) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c.trim(), a.trim()),
        None => (line, ""),
    };

    match cmd {
        "ping" => PONG.to_string(),
        "status" => {
            let topics_cnt = s.topics.all_mappings().len();
            format!("OK: herdr-telegram running, {topics_cnt} topic(s) mapped\n")
        }
        "topics" => report_topics(s).await,
        "reset" => {
            if args.is_empty() {
                USAGE_RESET.to_string()
            } else {
                // chat=0: silent CLI reply only — never spam the forum
                // General with `✅ Reset…` confirmations.
                match run_single_topic_reset(s, 0, None, args).await {
                    Ok(msg) => format!("OK: {msg}\n"),
                    Err(e) => format!("ERR: {e}\n"),
                }
            }
        }
        "trigger" => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() < 2 {
                USAGE_TRIGGER.to_string()
            } else {
                let (pane, status) = (parts[0], parts[1]);
                // Fail-closed: junk statuses must not flow into observers.
                if !matches!(status, "blocked" | "working" | "done" | "idle" | "shell") {
                    return USAGE_TRIGGER_STATUS.to_string();
                }
                observe_status(s, pane, status, false, "ctl").await;
                format!("OK: triggered '{status}' on pane '{pane}'\n")
            }
        }
        "inspect" => {
            if args.is_empty() {
                USAGE_INSPECT.to_string()
            } else {
                inspect_pane(s, args).await
            }
        }
        _ => UNKNOWN_CTL.to_string(),
    }
}

async fn report_topics(s: &AppState) -> String {
    let mappings = s.topics.all_mappings();
    let (live_panes, live_err) = match list_panes(&s.cfg.socket).await {
        Ok(l) => (l, false),
        Err(_) => (Vec::new(), true),
    };
    let live_set: HashSet<String> = live_panes.iter().cloned().collect();
    let (spaces, spaces_err) = match list_workspaces(&s.cfg.socket).await {
        Ok(l) => (l, false),
        Err(_) => (Vec::new(), true),
    };
    let (facts, facts_err) = match pane_facts(&s.cfg.socket).await {
        Ok(m) => (m, false),
        Err(_) => (std::collections::HashMap::new(), true),
    };
    let (tabs, tabs_err) = match tab_labels(&s.cfg.socket).await {
        Ok(m) => (m, false),
        Err(_) => (std::collections::HashMap::new(), true),
    };
    let mut out = String::new();
    if let Some(banner) = degraded_banner(live_err, spaces_err, facts_err, tabs_err) {
        out.push_str(banner);
    }
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
    let space = pf.map(|f| ws_label(&spaces, &f.ws)).unwrap_or("?");
    let multi = !tab_id.is_empty()
        && tab_id != "-"
        && facts.values().filter(|f| f.tab_id == tab_id).count() > 1;
    let tag_str = tag.as_deref().unwrap_or("?");
    let core = title_core_for(
        if tab_label == "-" {
            None
        } else {
            Some(tab_label)
        },
        tag_str,
        multi,
        pf.and_then(|f| f.label.as_deref()),
    )
    .unwrap_or_else(|| tag_str.to_string());
    let desired = format_title(space, &core, kind);

    format!(
        "=== PANE INSPECTION: {pane} ===\n\
         Thread ID:   {}\n\
         Topic Title: {} (stored)\n\
         Desired:     {desired} (tab wins, terminal→card only)\n\
         Tag:         {}\n\
         Tab:         {tab_label} ({tab_id})\n\
         Pane Label:  {}\n\
         Workspace:   {ws_name}\n\
         Agent Kind:  {kind} (icon only)\n\
         Term Title:  {term_title} (identity card only)\n\
         Status:      {}\n\
         Recent Msgs: {:?}\n\
         ==============================\n",
        th.map(|t| format!("#{t}")).unwrap_or_else(|| "none".into()),
        title.as_deref().unwrap_or("-"),
        tag.as_deref().unwrap_or("-"),
        pf.and_then(|f| f.label.as_deref()).unwrap_or("-"),
        agent.as_ref().map(|a| a.status.as_str()).unwrap_or("ready"),
        recent
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::cancel::isolated_state;

    #[tokio::test]
    async fn test_dispatch_ping_unknown_and_usage() {
        // Isolated state (no live mappings, no socket): pure dispatch
        // branches only — success paths need herdr RPCs.
        let (s, _dir) = isolated_state();
        assert_eq!(handle_cmd(&s, "ping").await, PONG);
        assert_eq!(handle_cmd(&s, "bogus").await, UNKNOWN_CTL);
        // Whitespace-tolerant split, usage before any I/O.
        assert_eq!(handle_cmd(&s, "  reset  ").await, USAGE_RESET);
        assert_eq!(handle_cmd(&s, "trigger w1:p1").await, USAGE_TRIGGER);
        // Junk statuses refuse before any observer I/O (fail-closed).
        assert_eq!(
            handle_cmd(&s, "trigger w1:p1 frobnicate").await,
            USAGE_TRIGGER_STATUS
        );
        assert_eq!(handle_cmd(&s, "inspect").await, USAGE_INSPECT);
        // Status shape (count varies with live data — assert the frame).
        let st = handle_cmd(&s, "status").await;
        assert!(st.starts_with("OK: herdr-telegram running, "));
        assert!(st.ends_with(" topic(s) mapped\n"));
    }

    #[test]
    fn test_degraded_banner_any_err_stales() {
        // Any single failed herdr read stales the table; all-ok is quiet.
        assert_eq!(degraded_banner(false, false, false, false), None);
        assert_eq!(
            degraded_banner(true, false, false, false),
            Some(DEGRADED_BANNER)
        );
        assert_eq!(
            degraded_banner(false, true, false, false),
            Some(DEGRADED_BANNER)
        );
        assert_eq!(
            degraded_banner(false, false, true, false),
            Some(DEGRADED_BANNER)
        );
        assert_eq!(
            degraded_banner(false, false, false, true),
            Some(DEGRADED_BANNER)
        );
        assert_eq!(
            degraded_banner(true, true, true, true),
            Some(DEGRADED_BANNER)
        );
    }
}
