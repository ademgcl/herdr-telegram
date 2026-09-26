//! Control-socket command implementation: inspection, single-topic
//! reset, and event mocking. Split from `ctl` (transport lives there).
//! Pure-dispatch branches (ping/unknown/usage) are unit-tested; success
//! paths need a live herdr socket.
use crate::{
    handlers::reset::run_single_topic_reset,
    herdr::{
        client::{list_panes, list_workspaces},
        labels::{pane_facts, tab_labels},
    },
    notifier::status::observe_status,
    state::AppState,
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
            // Exact arity: trailing junk must not silently pass (fail-closed
            // parity with junk statuses below).
            if parts.len() != 2 {
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
            // Exact arity (trigger parity): trailing junk must not flow
            // into the pane inspect (fail-closed).
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() != 1 {
                USAGE_INSPECT.to_string()
            } else {
                crate::ctl_inspect::inspect_pane(s, parts[0]).await
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
        // Trailing junk refuses too (exact arity, never silent OK).
        assert_eq!(
            handle_cmd(&s, "trigger w1:p1 blocked junk").await,
            USAGE_TRIGGER
        );
        assert_eq!(handle_cmd(&s, "inspect").await, USAGE_INSPECT);
        // Trailing junk refuses too (exact arity, never silent OK).
        assert_eq!(handle_cmd(&s, "inspect w1:p1 junk").await, USAGE_INSPECT);
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
