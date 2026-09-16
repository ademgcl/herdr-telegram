use crate::{
    handlers::dialog::{refresh_blocked_card, send_blocked_card},
    herdr::client::{get_agent, list_workspaces, read_agent_output, read_screen_visible},
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    notifier::cards::post_spontaneous_card,
    state::AppState,
    ui::ws_label,
};
use std::time::{Duration, Instant};

/// How long after a prompt's final card an idle/done alert is redundant.
const POST_PROMPT_QUIET_SECS: u64 = 45;
/// done↔idle bounces closer than this are flap (collapsed); slower ones
/// are legitimate sampled completions.
const FLAP_WINDOW_SECS: u64 = 15;

pub async fn observe_status(s: &AppState, pane: &str, new_status: &str, silent: bool, src: &str) {
    // Observation instant for debounce-style suppression: a final stamped
    // after this (during the RPCs below) makes our pre-RPC delta stale →
    // suppress instead of double-posting it.
    let observed_at = Instant::now();
    // Atomic status+last_change (lock order status→last_change, never
    // inverted anywhere — same pattern as torder→targets): concurrent
    // observes can't pair old from tick A with prev_change from tick B.
    let (old, prev_change) = {
        let mut st = s.status.lock().await;
        let mut lc = s.last_change.lock().await;
        let old = st.insert(pane.to_string(), new_status.to_string());
        let prev = lc.insert(pane.to_string(), std::time::Instant::now());
        (old, prev)
    };

    // Collapse rapid done <-> idle flap up front — before any fetch,
    // so oscillation never costs RPCs. Collapsed returns skip the
    // typing touch below: correct (job-owned keeps watcher typing;
    // job-less has no task), and a perpetual fast flap suppressing
    // forever is intended (it never did work between samples).
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

    // Leaving blocked behind — its dialog sig dies with the episode so
    // the next block episode always posts.
    if old.as_deref() == Some("blocked") && new_status != "blocked" {
        s.blocked_sig.lock().await.remove(pane);
    }
    // Leaving working behind — any stall episode (limit alert, stuck
    // timer, absence streak) dies with it so the next working stall
    // always re-alerts fresh instead of being suppressed by the prior
    // episode's 30-min remind window / stale stuck timer.
    if old.as_deref() == Some("working") && new_status != "working" {
        s.clear_limit_episode(pane).await;
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

    // The pane's topic exists (ensured silently — never notifies).
    // Reset sırasında atlanır: event-driven ensure, reset'in sildiği
    // topic'i anında yeniden açıp sil/oluştur ile yarışmasın. Hafıza
    // (status/last_change yukarıda) güncellenmeye devam eder.
    if !crate::handlers::reset::is_resetting() {
        s.topics.sync_topic(pane, &kind, raw_space).await;

        if let Some(forum) = s.cfg.forum {
            let branch = info.as_ref().and_then(|a| a.branch.as_deref());
            let title_opt = if title.trim().is_empty() {
                None
            } else {
                Some(title.as_str())
            };
            let card = crate::ui::build_pinned_card_text(
                &kind, pane, raw_space, new_status, title_opt, branch,
            );
            let mut mid_opt = s.topics.get_pin(pane);
            if let Some(mid) = mid_opt {
                if s.tg.try_edit_msg(forum, mid, &card, None).await.is_err() {
                    mid_opt = None;
                }
            }
            if mid_opt.is_none()
                && let Some(thread) = s.topics.all_mappings().get(pane).copied()
                && let Some(new_mid) = s.tg.send_msg(forum, Some(thread), &card, None).await
            {
                let _ = s.tg.pin_msg(forum, new_mid).await;
                s.topics.set_pin(pane, new_mid);
            }
        }
    }

    // F11: writing/typing indicator & F1: reopen topic when working.
    // A job-owned pane keeps its task across settled samples (the
    // watcher stops it at retire): stopping here would kill mid-job
    // typing on every idle sample between turns.
    if new_status == "working" {
        if !crate::handlers::reset::is_resetting() {
            s.topics.reopen_topic(pane).await;
            s.start_typing(pane).await;
        }
    } else {
        s.stop_typing_unless_owned(pane).await;
    }

    // Single jobs snapshot for the whole observe (blocked repeat,
    // blocked transition, and job-owned early-return below all reuse
    // it): a job appearing mid-observe races the watcher's finalize
    // into a double-card, mitigated by settle_check's re-check +
    // last_done (documented TOCTOU, not closed).
    let job_owned = s.jobs.lock().await.contains_key(pane);

    if silent {
        // Boot seed only — but an already-blocked pane genuinely needs
        // input NOW (missed while the bot was down): post its answer card
        // instead of staying mute until the next transition.
        if old.is_none() && new_status == "blocked" {
            println!("[alert] seed found {pane} blocked — posting answer card");
            let screen = read_screen_visible(&s.cfg.socket, pane, 80).await;
            // Baseline follows delivery: a dropped seed card must stay
            // "new" so the next observation posts it.
            let posted = if let Some(forum) = s.cfg.forum {
                let thread = s.topics.all_mappings().get(pane).copied();
                send_blocked_card(s, forum, thread, pane).await
            } else {
                let mut posted = false;
                for id in &s.cfg.owners {
                    if send_blocked_card(s, *id, None, pane).await {
                        posted = true;
                    }
                }
                posted
            };
            if posted {
                s.seen.lock().await.insert(pane.to_string(), screen);
            }
        }
        return;
    }
    if old.as_deref() == Some(new_status) {
        // Same status twice — except blocked: consecutive dialogs turn
        // over with NO transition (allow → confirm), so content, not the
        // transition, decides whether a card is due (job_owned from the
        // single snapshot above).
        if new_status == "blocked" && !job_owned {
            refresh_blocked_card(s, pane).await;
        }
        return;
    }

    // A prompt job owns this pane — covered by its watcher (kept here too
    // for the transition path below, mirroring the repeat path above).
    if new_status == "blocked" && !job_owned {
        refresh_blocked_card(s, pane).await;
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

    // Re-entered an agent in a shelled pane: the user did it by hand in
    // the topic, so it needs no announcement. Blocked still surfaces.
    if old.as_deref() == Some("shell") && matches!(new_status, "idle" | "done") {
        println!("[alert] suppressed re-enter {new_status} for {pane} ({src})");
        s.seen.lock().await.insert(pane.to_string(), screen);
        return;
    }

    // A prompt job owns this pane — the watcher's live message / final
    // card covers it. (Seen is anchored by the job's finalize, so don't
    // consume here.) Reuses the snapshot above — same documented TOCTOU.
    if job_owned {
        return;
    }

    // Settle→settle bounce right after a prompt's final card carries no
    // fresh work — consume the baseline and stay quiet. Blocked is
    // excluded: input needed NOW always surfaces, never suppressed.
    let fresh_work = matches!(old.as_deref(), None | Some("working"));
    if !fresh_work
        && fresh_body.is_empty()
        && matches!(new_status, "idle" | "done")
        && let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
    {
        println!("[alert] suppressed post-prompt {new_status} for {pane} ({src})");
        s.seen.lock().await.insert(pane.to_string(), screen);
        return;
    }

    println!("[alert] {src}: {pane} {old:?}→{new_status}");

    // DM mode has no topics — legacy immediate pushes.
    if s.cfg.forum.is_none() {
        crate::notifier::dm::push_dm_alert(
            s,
            pane,
            &kind,
            raw_space,
            &space_label,
            &title,
            new_status,
            &screen,
            observed_at,
        )
        .await;
        return;
    }

    // Forum mode: blocked cards are handled up front (content-addressed,
    // so turned-over dialogs surface). done/idle arm the debounce: the
    // push waits to confirm the settle isn't mid-task flicker.
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
