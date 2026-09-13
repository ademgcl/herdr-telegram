use std::time::{Duration, Instant};
use serde_json::json;
use crate::{
    handlers::interactive::send_blocked_card,
    herdr::client::{get_agent, list_workspaces, read_agent_output, read_screen_visible},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    notifier::cards::post_spontaneous_card,
    state::AppState,
    ui::{btn, emoji, ws_label},
};

/// How long after a prompt's final card an idle/done alert is redundant.
const POST_PROMPT_QUIET_SECS: u64 = 45;
/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions.
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

    // Collapse rapid done <-> idle flap up front — before any fetch or
    // icon touch, so oscillation neither costs RPCs nor discoes the icon.
    // Slow sampled bounces are legitimate completions: the agent did work
    // between observations, so they flow through.
    if ((old.as_deref() == Some("done") && new_status == "idle")
        || (old.as_deref() == Some("idle") && new_status == "done"))
        && prev_change
            .map(|t| t.elapsed() < Duration::from_secs(FLAP_WINDOW_SECS))
            .unwrap_or(false)
    {
        println!("[alert] collapsed {old:?}→{new_status} for {pane} ({src})");
        return;
    }

    // Fresh settles restart the done→idle display decay clock.
    if old.as_deref() != Some(new_status)
        && matches!(new_status, "idle" | "done" | "blocked")
    {
        s.settled_at
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
    }

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

    // The pane's topic exists and its state icon tracks the DISPLAY
    // status — long-quiet completions show idle (available again) while
    // herdr-truth stays done everywhere else. Silent, instant, zero
    // clutter: edits and icon swaps never notify, only pushes buzz.
    let settled_age = s
        .settled_at
        .lock()
        .await
        .get(pane)
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    let display = crate::topics::names::display_status(new_status, settled_age);
    s.topics
        .sync_topic(pane, &kind, raw_space, display)
        .await;

    if silent {
        // Boot seed only — but an already-blocked pane genuinely needs
        // input NOW (missed while the bot was down): post its answer card
        // instead of staying mute until the next transition.
        if old.is_none() && new_status == "blocked" {
            println!("[alert] seed found {pane} blocked — posting answer card");
            let screen = read_screen_visible(&s.cfg.socket, pane, 80).await;
            s.seen.lock().await.insert(pane.to_string(), screen);
            if let Some(forum) = s.cfg.forum {
                let thread = s.topics.all_mappings().get(pane).copied();
                send_blocked_card(&s, forum, thread, pane).await;
            } else {
                for id in &s.cfg.owners {
                    send_blocked_card(&s, *id, None, pane).await;
                }
            }
        }
        return;
    }
    if old.as_deref() == Some(new_status) {
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

    // A prompt job owns this pane — the watcher's live message / final
    // card covers it. (Seen is anchored by the job's finalize, so don't
    // consume here.)
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }

    // Settle→settle bounce right after a prompt's final card carries no
    // fresh work — consume the baseline and stay quiet. Blocked is
    // excluded: input needed NOW always surfaces, never suppressed.
    let fresh_work = matches!(old.as_deref(), None | Some("working"));
    if !fresh_work
        && matches!(new_status, "idle" | "done")
        && let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
    {
        println!("[alert] suppressed post-prompt {new_status} for {pane} ({src})");
        s.seen.lock().await.insert(pane.to_string(), screen);
        return;
    }

    println!("[alert] {src}: {pane} {old:?}→{new_status}");

    // DM mode has no topics — legacy immediate pushes, but blocked always
    // gets the answer card (buttons work in DMs too).
    if s.cfg.forum.is_none() {
        s.seen.lock().await.insert(pane.to_string(), screen);
        if new_status == "blocked" {
            for id in &s.cfg.owners {
                send_blocked_card(&s, *id, None, pane).await;
            }
            return;
        }
        if !fresh_body.is_empty() {
            post_spontaneous_card(&s, pane, &kind, raw_space, new_status, &fresh_body).await;
            return;
        }
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

    // Forum mode: blocked needs input NOW — always post its answer card
    // (exactly one per block episode: repeats return early on old==new).
    // done/idle arm the debounce: the push waits to confirm the settle
    // isn't mid-task flicker.
    if new_status == "blocked" {
        s.seen.lock().await.insert(pane.to_string(), screen);
        if let Some(forum) = s.cfg.forum {
            let thread = s.topics.all_mappings().get(pane).copied();
            send_blocked_card(&s, forum, thread, pane).await;
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
        crate::notifier::cards::settle_check(s2, pane2, st2, at).await;
    });
}
