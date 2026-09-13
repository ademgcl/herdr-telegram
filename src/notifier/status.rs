use std::time::Duration;
use serde_json::json;
use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{btn, chunks, emoji, ws_label},
};

/// How long after a prompt's final card an idle/done alert is redundant.
const POST_PROMPT_QUIET_SECS: u64 = 45;
/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions (posted).
const FLAP_WINDOW_SECS: u64 = 15;

pub async fn observe_status(
    s: &AppState,
    pane: &str,
    new_status: &str,
    silent: bool,
    src: &str,
) {
    let old = {
        let mut m = s.status.lock().await;
        m.insert(pane.to_string(), new_status.to_string())
    };
    let prev_change = {
        let mut m = s.last_change.lock().await;
        m.insert(pane.to_string(), std::time::Instant::now())
    };

    if silent || old.as_deref() == Some(new_status) {
        return;
    }

    let is_attention = matches!(new_status, "blocked" | "done" | "idle");
    if !is_attention {
        return;
    }

    // Collapse rapid done <-> idle flap — but only when genuinely rapid.
    // Slow sampled bounces (watchdog) are legitimate completions: the
    // agent did work between observations, so they must post.
    if ((old.as_deref() == Some("done") && new_status == "idle")
        || (old.as_deref() == Some("idle") && new_status == "done"))
        && prev_change
            .map(|t| t.elapsed() < Duration::from_secs(FLAP_WINDOW_SECS))
            .unwrap_or(false)
    {
        println!("[alert] collapsed {old:?}→{new_status} for {pane} ({src})");
        return;
    }

    // Suppress parallel alert if active prompt job is running —
    // the watcher's live message / final card already covers this pane.
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }

    // Suppress the redundant idle/done echo right after a prompt's final
    // card — but ONLY for settle→settle bounces with no fresh work. A new
    // work cycle (old == working) or a first sighting always posts, so a
    // local reply right after a Telegram card is never swallowed.
    let fresh_work = matches!(old.as_deref(), None | Some("working"));
    if !fresh_work
        && matches!(new_status, "idle" | "done" | "blocked")
        && let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
    {
        println!("[alert] suppressed post-prompt {new_status} for {pane} ({src})");
        return;
    }

    println!("[alert] {src}: {pane} {old:?}→{new_status}");

    // Spontaneous output (no Telegram job prompted this pane — e.g. the
    // owner typed directly in the terminal): forward the fresh reply to
    // the agent's topic with the same extractor as prompt cards, so both
    // directions land in Telegram. Idempotent via the seen baseline:
    // settles with no fresh delta fall through to the short alert.
    // First sight only anchors the baseline — never posts stale scrollback.
    let screen: Vec<String> = read_agent_output(&s.cfg.socket, pane, 80)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let mut seen = s.seen.lock().await;
    let base = seen.get(pane).cloned().unwrap_or_default();
    // First sight has no baseline: extract from the whole screen. This
    // only runs on a real transition, so the last segment is fresh work
    // that completed while the bot was up — never stale scrollback.
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let fresh_body = join_trimmed(&final_block(&source, ""));
    seen.insert(pane.to_string(), screen);
    drop(seen);

    if !fresh_body.is_empty() {
        post_spontaneous_card(&s, pane, new_status, &fresh_body).await;
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
        return;
    }

    let info = get_agent(&s.cfg.socket, pane).await.ok();
    let (kind, ws_id, title) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone(), a.title.clone()),
        None => ("?".into(), "?".into(), String::new()),
    };

    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    let space_label = spaces
        .iter()
        .find(|w| w.id == ws_id)
        .map(|w| format!("#{} {}", w.number, w.label))
        .unwrap_or_else(|| ws_id.clone());

    let hint = match new_status {
        "blocked" => "\n↩️ reply or type in topic to answer",
        _ => "",
    };
    let verb = if new_status == "idle" { "ready" } else { new_status };
    let mut text = format!("{} {}: {kind} @ {space_label}", emoji(new_status), verb);
    if !title.is_empty() {
        let short: String = title.chars().take(60).collect();
        text.push_str(&format!("\n{short}"));
    }
    // NOTE: deliberately NO screen tail here. herdr only exposes raw TUI
    // text (box-drawing footers, Thought headers, tool echoes) for every
    // provider — dumping it as an alert is pure boilerplate. The final
    // prompt card carries the answer; `/read` shows raw output on demand.
    text.push_str(hint);

    // NOTE: deliberately NOT touching focus here — background alerts must never
    // hijack where the owner's next plain-text message gets delivered.

    // Alerts belong WHERE THE AGENT LIVES: its topic, as plain chat text.
    // Direct messages are only for non-forum setups. Never both.
    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.ensure_topic(pane, &kind, raw_space).await {
            let mid = s.tg.send_msg(forum, Some(thread), &text, None).await;
            s.remember(forum, mid, pane).await;
        }
    } else {
        for id in &s.cfg.owners {
            let mid = s.tg
                .send_msg(
                    *id,
                    None,
                    &text,
                    Some(json!([[btn("show output", &format!("o:{pane}"))]])),
                )
                .await;
            s.remember(*id, mid, pane).await;
        }
    }
}

/// Spontaneous settle card: same shape as a prompt final card, for output
/// the bot didn't ask for (local typing, background work). Routes to the
/// agent's topic — never hijacks focus.
async fn post_spontaneous_card(s: &AppState, pane: &str, settled: &str, body: &str) {
    let info = get_agent(&s.cfg.socket, pane).await.ok();
    let (kind, ws_id) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone()),
        None => ("?".into(), "?".into()),
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);

    let hint = match settled {
        "blocked" => "\n↩️ reply or type in topic to answer",
        _ => "",
    };
    let header = format!("{} {settled}", emoji(settled));
    let parts = chunks(&format!("{header}\n\n{body}{hint}"), MAX_MSG_UNITS);
    println!("[alert] spontaneous card {pane}: {} part(s), body {} chars", parts.len(), body.len());

    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.ensure_topic(pane, &kind, raw_space).await {
            for part in &parts {
                let mid = s.tg.send_msg(forum, Some(thread), part, None).await;
                s.remember(forum, mid, pane).await;
            }
        }
    } else {
        for id in &s.cfg.owners {
            for part in &parts {
                let mid = s.tg.send_msg(*id, None, part, None).await;
                s.remember(*id, mid, pane).await;
            }
        }
    }
}
