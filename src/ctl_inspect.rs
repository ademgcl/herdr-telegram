//! `inspect <pane>` detail view: split from `ctl_cmd` (300-line file limit).
use crate::{
    ctl_cmd::{DEGRADED_BANNER, degraded_banner},
    handlers::title_rules::title_core_for,
    herdr::{
        client::{get_agent, list_workspaces},
        labels::{pane_facts, tab_labels},
    },
    state::AppState,
    topics::names::format_title,
    ui::ws_label,
};

pub(crate) async fn inspect_pane(s: &AppState, pane: &str) -> String {
    let (facts, facts_err) = match pane_facts(&s.cfg.socket).await {
        Ok(m) => (m, false),
        Err(_) => (std::collections::HashMap::new(), true),
    };
    let (tabs, tabs_err) = match tab_labels(&s.cfg.socket).await {
        Ok(m) => (m, false),
        Err(_) => (std::collections::HashMap::new(), true),
    };
    let (spaces, spaces_err) = match list_workspaces(&s.cfg.socket).await {
        Ok(l) => (l, false),
        Err(_) => (Vec::new(), true),
    };
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
    // Fail-closed on sick herdr: unknown kind/status read stale, never a
    // healthy `shell`/`ready` (that fakes a live shell over an outage).
    let kind = agent.as_ref().map(|a| a.kind.as_str()).unwrap_or("?");
    let status = agent.as_ref().map(|a| a.status.as_str()).unwrap_or("? (stale)");
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

    let mut out = String::new();
    // `report_topics` parity: a sick herdr must read stale, never
    // authoritative (unwrap_or_default alone fakes `-`/`ready`). The
    // agent read failing (kind/status "?" above) raises it like any
    // other herdr read error.
    if degraded_banner(agent.is_none(), spaces_err, facts_err, tabs_err).is_some() {
        out.push_str(DEGRADED_BANNER);
    }
    out.push_str(&format!(
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
        status,
        recent
    ));
    out
}
