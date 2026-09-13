use std::sync::Arc;
use serde_json::json;
use tokio::time::{Duration, Instant};
use crate::{
    handlers::interactive::send_blocked_card,
    herdr::client::{get_agent, read_screen, rpc_t},
    jobs::finalize::{edit_live, finalize, report},
    jobs::job::Job,
    jobs::segment::final_block,
    jobs::stream::{delta, EvStream, WatchEvent},
    state::AppState,
    types::{AgentRow, PromptRequest, LIVE_EDIT_COOLDOWN_SECS},
    ui::tail_fit,
};
use crate::herdr::client::read_screen_adaptive;

/// Terminal statuses that end a watch cycle.
const SETTLED: &[&str] = &["idle", "done", "blocked", "exited", "closed", "dead"];
/// Safety-net tick in case herdr events are unavailable.
const FALLBACK_TICK_SECS: u64 = 5;
/// Min gap between event-socket reconnect attempts (prevents tight-loop starvation).
const REOPEN_COOLDOWN_SECS: u64 = 5;

pub async fn enqueue_prompt(
    s: AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    row: AgentRow,
    text: String,
) {
    let req = PromptRequest {
        chat_id,
        message_thread_id: thread_id,
        text,
    };
    let pane = row.pane.clone();

    let existing = s.jobs.lock().await.get(&pane).cloned();
    let job = match existing {
        Some(j) => j,
        None => {
            let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
            let j = Job::new(baseline, chat_id, thread_id);
            s.jobs.lock().await.insert(pane.clone(), j.clone());
            tokio::spawn(watch_job(s.clone(), pane.clone(), j.clone()));
            j
        }
    };

    *job.dest.lock().await = (req.chat_id, req.message_thread_id);
    *job.prompt.lock().await = req.text.clone();
    *job.pending.lock().await += 1;
    s.set_focus(&pane).await;

    // Deliver immediately — interactive agents buffer input like a real terminal
    if let Err(e) = rpc_t(
        &s.cfg.socket,
        "agent.prompt",
        json!({"target": pane, "text": req.text}),
        30,
    )
    .await
    {
        println!("[jobs] submit error: {e}");
        *job.pending.lock().await -= 1;
        // Retire the watcher: nothing was delivered, so it must not report.
        // (Without this it finalizes on the untouched screen — the bogus
        // "(no captured output)" card.) Blocked panes get the interactive
        // card instead, so replying always works.
        job.mark_stopped();
        job.cancel.notify_waiters();
        let msg = e.to_string();
        if msg.contains("blocked") {
            send_blocked_card(
                &s,
                req.chat_id,
                req.message_thread_id,
                &pane,
            )
            .await;
        } else {
            report(&s, req.chat_id, req.message_thread_id, &pane, &format!("⚠️ error: {e}")).await;
        }
    }
}

/// Watch the agent via herdr push-events: every output burst updates one live
/// Telegram message; settle turns it into the final result card.
async fn watch_job(s: AppState, pane: String, job: Arc<Job>) {
    let mut live_mid: Option<i64> = None;
    let mut last_edit = Instant::now() - Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS);
    // Raw output since the prompt — the fresh reply is extracted from
    // this at display time (last segment only, see segment::final_block)
    let mut acc: Vec<String> = Vec::new();
    let mut ev = None;
    let mut last_open = Instant::now() - Duration::from_secs(REOPEN_COOLDOWN_SECS);
    println!("[watcher] start {pane}");

    loop {
        if job.is_stopped() {
            break;
        }

        // Reconnect the event stream lazily — never in a hot loop
        if ev.is_none() && last_open.elapsed() >= Duration::from_secs(REOPEN_COOLDOWN_SECS) {
            last_open = Instant::now();
            ev = EvStream::open(&s.cfg.socket, &pane).await.ok();
        }

        // Output activity → stream; status change → maybe finalize.
        // The fallback tick guarantees progress even without events.
        // Events (when they fire) simply trigger an earlier wake-up
        let _event = tokio::select! {
            _ = job.cancel.notified() => {
                job.mark_stopped();
                let (chat, th) = *job.dest.lock().await;
                edit_live(&s, chat, th, &pane, &mut live_mid, "✋ cancelled").await;
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(FALLBACK_TICK_SECS)) => WatchEvent::Output,
            e = async {
                match ev.as_mut() {
                    Some(stream) => stream.next().await,
                    None => std::future::pending().await,
                }
            } => match e {
                Some(w) => w,
                None => { ev = None; continue; }
            },
        };

        // Every wake-up: check settle first (never depend on herdr events),
        // then stream whatever output is new.
        let Ok(agent) = get_agent(&s.cfg.socket, &pane).await else { continue };
        if SETTLED.contains(&agent.status.as_str()) {
            // Collapse done↔idle flapping before committing to a report
            tokio::time::sleep(Duration::from_millis(750)).await;
            if let Ok(a) = get_agent(&s.cfg.socket, &pane).await
                && a.status == "working"
            {
                continue;
            }
            finalize(&s, &pane, &job, &agent.status, &mut live_mid, &mut acc).await;
            break;
        }

        // Stream whatever is new into the live message
        if last_edit.elapsed() < Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS) {
            continue;
        }
        let screen = read_screen_adaptive(&s.cfg.socket, &pane).await;
        if screen.is_empty() {
            continue; // nothing readable yet — try next wake-up
        }
        if !job.baseline_ok() {
            job.anchor_baseline(screen).await;
            continue;
        }
        let base = job.baseline.lock().await.clone();
        let fresh = delta(&screen, &base);
        if fresh.is_empty() {
            continue;
        }
    // Raw accumulation: boundaries (tool echoes, headers, prompt echo)
    // are resolved at display time so only the fresh reply is shown.
        acc.extend(fresh.iter().cloned());
        if acc.len() > 400 {
            let drop = acc.len() - 400;
            acc.drain(..drop);
        }
        *job.baseline.lock().await = screen;

        let prompt = job.prompt.lock().await.clone();
        let seg = final_block(&acc, &prompt);
        if seg.is_empty() {
            continue; // chrome-only so far — nothing worth showing yet
        }
        let (chat, th) = *job.dest.lock().await;
        s.tg.typing(chat, th).await;
        let text = format!("🔄 working…\n\n{}", tail_fit(&seg, 3200));
        match live_mid {
            Some(mid) => s.tg.edit_msg(chat, mid, &text, None).await,
            None => live_mid = s.tg.send_msg(chat, th, &text, None).await,
        }
        last_edit = Instant::now();
    }

    // Retire only if the map still points at THIS watcher (no newer job took over)
    let mut map = s.jobs.lock().await;
    if map.get(&pane).map(|j| Arc::ptr_eq(j, &job)).unwrap_or(false) {
        map.remove(&pane);
    }
}
