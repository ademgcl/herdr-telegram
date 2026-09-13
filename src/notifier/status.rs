use std::time::{Duration, Instant};
use serde_json::json;
use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    notifier::pin::refresh_pin,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::{btn, chunks, emoji, ws_label},
};

/// How long after a prompt's final card an idle/done alert is redundant.
const POST_PROMPT_QUIET_SECS: u64 = 45;
/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions.
const FLAP_WINDOW_SECS: u64 = 15;
/// A settle must hold this long before a spontaneous answer pushes —
/// micro-settle flicker mid-task stays on the pin instead of buzzing.
/// Blocked (needs input) always pushes immediately.
const SETTLE_DEBOUNCE_SECS: u64 = 15;

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

    // Agent identity once per observation — shared by pins, cards and
    // alerts below.
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

    // The pane's topic exists (static `{tag} · {space}` title, set once).
    s.topics.ensure_topic(pane, &kind, raw_space).await;
    // The pinned card always tracks status — even seeds and working
    // transitions. Silent, in-place, zero message cost.
    refresh_pin(s, pane, new_status).await;

    if silent || old.as_deref() == Some(new_status) {
        return;
    }

    let is_attention = matches!(new_status, "blocked" | "done" | "idle");
    if !is_attention {
        return;
    }

    // Fresh screen vs the seen baseline. First sight extracts from the
    // whole screen — this only runs on a real transition, so the last
    // segment is freshly completed work.
    let screen: Vec<String> = read_agent_output(&s.cfg.socket, pane, 80)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let base = s.seen.lock().await.get(pane).cloned().unwrap_or_default();
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let fresh_body = join_trimmed(&final_block(&source, ""));
    if !fresh_body.is_empty() {
        s.last_reply
            .lock()
            .await
            .insert(pane.to_string(), fresh_body.clone());
        refresh_pin(s, pane, new_status).await;
    }

    // Collapse rapid done <-> idle flap — but only when genuinely rapid.
    if ((old.as_deref() == Some("done") && new_status == "idle")
        || (old.as_deref() == Some("idle") && new_status == "done"))
        && prev_change
            .map(|t| t.elapsed() < Duration::from_secs(FLAP_WINDOW_SECS))
            .unwrap_or(false)
    {
        println!("[alert] collapsed {old:?}→{new_status} for {pane} ({src})");
        s.seen.lock().await.insert(pane.to_string(), screen);
        return;
    }

    // A prompt job owns this pane — the watcher's live message / final
    // card covers it. (Pin already refreshed above; seen is anchored by
    // the job's finalize, so don't consume here.)
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }

    // Settle→settle bounce right after a prompt's final card carries no
    // fresh work — consume the baseline and stay quiet.
    let fresh_work = matches!(old.as_deref(), None | Some("working"));
    if !fresh_work
        && matches!(new_status, "idle" | "done" | "blocked")
        && let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
    {
        println!("[alert] suppressed post-prompt {new_status} for {pane} ({src})");
        s.seen.lock().await.insert(pane.to_string(), screen);
        return;
    }

    println!("[alert] {src}: {pane} {old:?}→{new_status}");

    // DM mode has no topics or pins — legacy immediate pushes.
    if s.cfg.forum.is_none() {
        if !fresh_body.is_empty() {
            s.seen.lock().await.insert(pane.to_string(), screen);
            post_spontaneous_card(&s, pane, &kind, raw_space, new_status, &fresh_body).await;
            return;
        }
        s.seen.lock().await.insert(pane.to_string(), screen);
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
        text.push_str(hint);
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
        return;
    }

    // Forum mode: blocked needs input NOW — push immediately. done/idle
    // arm the debounce: the pin already shows state, the push waits to
    // confirm the settle isn't mid-task flicker.
    if new_status == "blocked" {
        s.seen.lock().await.insert(pane.to_string(), screen);
        if !fresh_body.is_empty() {
            post_spontaneous_card(&s, pane, &kind, raw_space, new_status, &fresh_body).await;
        }
        return;
    }
    let at = Instant::now();
    s.debounce
        .lock()
        .await
        .insert(pane.to_string(), (new_status.to_string(), at));
    println!("[alert] armed debounce {pane} → {new_status}");
    let s2 = s.clone();
    let pane2 = pane.to_string();
    let st2 = new_status.to_string();
    tokio::spawn(async move {
        settle_check(s2, pane2, st2, at).await;
    });
}

/// Debounced spontaneous push: posts the fresh reply only if this settle
/// is still current (no newer transition, no prompt takeover, no newer
/// card) after the grace period. Baseline is consumed either way.
async fn settle_check(s: AppState, pane: String, settled: String, armed_at: Instant) {
    tokio::time::sleep(Duration::from_secs(SETTLE_DEBOUNCE_SECS)).await;
    let current = s.debounce.lock().await.get(&pane).cloned();
    if current
        .map(|(st, at)| st != settled || at != armed_at)
        .unwrap_or(true)
    {
        return;
    }
    s.debounce.lock().await.remove(&pane);
    if s.jobs.lock().await.contains_key(&pane) {
        return;
    }
    if s.status
        .lock()
        .await
        .get(&pane)
        .map(|st| st != &settled)
        .unwrap_or(true)
    {
        return;
    }
    if s.last_done
        .lock()
        .await
        .get(&pane)
        .map(|t| *t > armed_at)
        .unwrap_or(false)
    {
        return;
    }
    let screen: Vec<String> = read_agent_output(&s.cfg.socket, &pane, 80)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let base = s.seen.lock().await.get(&pane).cloned().unwrap_or_default();
    let source: Vec<String> = if base.is_empty() {
        screen.clone()
    } else {
        delta(&screen, &base).to_vec()
    };
    let body = join_trimmed(&final_block(&source, ""));
    s.seen.lock().await.insert(pane.clone(), screen);
    if body.is_empty() {
        return;
    }
    let info = get_agent(&s.cfg.socket, &pane).await.ok();
    let (kind, ws_id) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone()),
        None => ("?".into(), "?".into()),
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    post_spontaneous_card(&s, &pane, &kind, raw_space, &settled, &body).await;
}

/// Answer push: the body alone (never a status-word lead), plus the reply
/// affordance when input is needed. Blocked keeps a ⛔ prefix for urgency.
async fn post_spontaneous_card(
    s: &AppState,
    pane: &str,
    kind: &str,
    space: &str,
    settled: &str,
    body: &str,
) {
    // NOTE: deliberately NOT touching focus here — background pushes must
    // never hijack where the owner's next plain-text message gets delivered.
    let text = match settled {
        "blocked" => format!("⛔ {body}\n↩️ reply or type in topic to answer"),
        _ => body.to_string(),
    };
    let parts = chunks(&text, MAX_MSG_UNITS);
    println!(
        "[alert] spontaneous card {pane}: {} part(s), body {} chars",
        parts.len(),
        body.len()
    );
    s.last_reply
        .lock()
        .await
        .insert(pane.to_string(), body.to_string());
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());
    refresh_pin(s, pane, settled).await;

    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.ensure_topic(pane, kind, space).await {
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
