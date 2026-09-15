use std::collections::HashSet;
use std::time::{Duration, Instant};
use crate::{
    handlers::titles::sync_titles,
    herdr::client::{list_agents, list_panes, read_screen_adaptive},
    jobs::notices::{detect_limit, limit_card_text},
    notifier::status::observe_status,
    state::AppState,
};

/// Re-remind while a background limit stall persists (prompt-owned
/// panes alert once per episode from their watcher instead).
const LIMIT_REMIND_SECS: u64 = 1800;

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Ok(rows) = list_agents(&s.cfg.socket).await else { return };

    let mut live_panes = HashSet::new();

    for r in &rows {
        live_panes.insert(r.pane.clone());
        // Silent or not, this ensures the topic + pin exist; non-silent
        // also posts cards for genuine transitions.
        observe_status(s, &r.pane, &r.status, silent, src).await;
    }

    // Rate-limit stalls never transition (herdr reports `working` while
    // opencode retries internally), so the status path above stays mute:
    // scan working panes for the banner directly. Skipped on the silent
    // seed (boot text can mimic error banners); the next watchdog tick
    // — 60s later — surfaces real stalls anyway.
    if !silent {
        scan_limits(s).await;
    }

    // Stored panes with no agent are shells (quit) or dead (closed).
    // Shells keep their topic with the shell badge and zero alerts;
    // only truly gone panes get closed.
    if s.cfg.forum.is_some() {
        let stored = s.topics.all_mappings();
        let missing: Vec<String> = stored
            .keys()
            .filter(|p| !live_panes.contains(*p))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let panes = list_panes(&s.cfg.socket).await.unwrap_or_default();
            for pane in missing {
                if panes.contains(&pane) {
                    s.status.lock().await.insert(pane.clone(), "shell".to_string());
                    s.topics.mark_shell(&pane).await;
                } else if !silent {
                    s.topics.close_topic(&pane).await;
                    s.topics.remove_mapping(&pane);
                    s.cancel_jobs_for(&pane).await;
                    s.clear_pane(&pane).await;
                }
            }
        }
    }

    // 1:1 pane↔topic titles (herdr labels win here; native TG renames
    // flow back via forum_topic_edited). Self-guards when forum is off.
    sync_titles(s).await;
}

/// Buzz once per limit episode on job-less working panes (prompt-owned
/// panes are the watcher's job — it replies in the prompt's own chat).
/// Clears the episode when the banner leaves so the next one re-alerts.
async fn scan_limits(s: &AppState) {
    let panes: Vec<String> = {
        let status = s.status.lock().await;
        let jobs = s.jobs.lock().await;
        status
            .iter()
            .filter(|(p, st)| st.as_str() == "working" && !jobs.contains_key(*p))
            .map(|(p, _)| p.clone())
            .collect()
    };
    for pane in panes {
        let screen = read_screen_adaptive(&s.cfg.socket, &pane).await;
        let Some(hit) = detect_limit(&screen) else {
            s.limit_alert.lock().await.remove(&pane);
            continue;
        };
        {
            let mut map = s.limit_alert.lock().await;
            let due = match map.get(&pane) {
                None => true,
                Some((k, t)) => {
                    k != hit.kind || t.elapsed() >= Duration::from_secs(LIMIT_REMIND_SECS)
                }
            };
            if !due {
                continue;
            }
            map.insert(pane.clone(), (hit.kind.to_string(), Instant::now()));
        }
        let text = limit_card_text(&pane, &hit);
        if let Some(forum) = s.cfg.forum {
            match s.topics.all_mappings().get(&pane).copied() {
                Some(thread) => {
                    let mid = s.tg.send_msg(forum, Some(thread), &text, None).await;
                    s.remember(forum, mid, &pane).await;
                }
                None => {
                    for id in &s.cfg.owners {
                        let mid = s.tg.send_msg(*id, None, &text, None).await;
                        s.remember(*id, mid, &pane).await;
                    }
                }
            }
        } else {
            for id in &s.cfg.owners {
                let mid = s.tg.send_msg(*id, None, &text, None).await;
                s.remember(*id, mid, &pane).await;
            }
        }
        println!("[alert] limit stall {pane}: {} ({})", hit.kind, hit.excerpt);
    }
}
