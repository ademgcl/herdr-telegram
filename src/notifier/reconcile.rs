use crate::{
    handlers::titles::sync_titles,
    herdr::client::{list_agents, list_panes, read_screen_adaptive},
    jobs::notices::{detect_limit, is_stuck_gated, limit_card_text},
    notifier::status::observe_status,
    state::AppState,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Re-remind while a background limit stall persists (prompt-owned
/// panes alert once per episode from their watcher instead).
pub const LIMIT_REMIND_SECS: u64 = 1800;
/// Gated (`provider`/`error`) banners must persist this long before the
/// watchdog buzzes: transient upstream blips (timeout → retry succeeds)
/// stay silent, stuck stalls page once.
const LIMIT_STUCK_SECS: u64 = 90;
/// Consecutive confirmed-clean 60s ticks before a limit episode clears.
/// A single scroll/RPC flap never re-arms the alert.
const LIMIT_CLEAR_MISSES: u32 = 2;

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Ok(rows) = list_agents(&s.cfg.socket).await else {
        return;
    };

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
                    s.status
                        .lock()
                        .await
                        .insert(pane.clone(), "shell".to_string());
                    // Agent gone (shell reuse): its stall episode dies here
                    // or the next agent on this pane name inherits stale
                    // dedup (see status.rs working→* clear).
                    s.clear_limit_episode(&pane).await;
                    s.topics.mark_shell(&pane).await;
                // Dead-pane close is silent (no card), so it never waits
                // for a non-silent tick — orphans from a restart close on
                // the seed pass instead of lingering a full cycle.
                } else {
                    if s.topics.close_topic(&pane).await {
                        s.topics.remove_mapping(&pane);
                    }
                    s.cancel_jobs_for(&pane).await;
                    s.clear_pane(&pane).await;
                }
            }
        }
    }

    // Mode-independent dead-pane hygiene: DM-mode prompts can orphan
    // jobs, durable intent, and per-pane maps for externally-closed
    // panes (no topic mapping exists to trigger the forum close flow).
    // Live panes are skipped; truly gone ones are cancelled + cleared.
    {
        let mut known: Vec<String> = s.jobs.lock().await.keys().cloned().collect();
        known.extend(s.pending.lock().await.keys().cloned());
        if !known.is_empty() {
            let live = list_panes(&s.cfg.socket).await.unwrap_or_default();
            for pane in known {
                if !live.contains(&pane) {
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
/// Once-per-episode survives noise: empty/outage reads preserve all
/// state (unknown ≠ clean), a banner must be absent for
/// [`LIMIT_CLEAR_MISSES`] consecutive clean reads to clear the episode,
/// gated (`provider`/`error`) banners must persist [`LIMIT_STUCK_SECS`]
/// before buzzing, and a recent alert suppresses re-pages regardless of
/// kind (scroll-order flips never spam; the 30-min remind still fires).
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
        // Outage/unknown: preserve everything (alert, stuck timer, miss
        // count) so the next good read does NOT re-alert.
        if screen.is_empty() {
            continue;
        }
        let Some(hit) = detect_limit(&screen) else {
            // Clean miss: only a sustained absence clears the episode.
            let misses = {
                let mut m = s.limit_miss.lock().await;
                let n = m.get(&pane).copied().unwrap_or(0) + 1;
                if n >= LIMIT_CLEAR_MISSES {
                    m.remove(&pane);
                } else {
                    m.insert(pane.clone(), n);
                }
                n
            };
            if misses >= LIMIT_CLEAR_MISSES {
                s.limit_alert.lock().await.remove(&pane);
                s.limit_seen.lock().await.remove(&pane);
            }
            continue;
        };
        // Banner present: absence streak over.
        s.limit_miss.lock().await.remove(&pane);
        // Stuck gate for transient-prone kinds: a timeout blip that
        // recovers on the next retry stays silent; only a banner that
        // persists across watchdog ticks pages. A kind flip restarts the
        // timer (co-present banners swapping topmost line never spam).
        if is_stuck_gated(hit.kind) {
            let stuck = {
                let mut seen = s.limit_seen.lock().await;
                match seen.get(&pane) {
                    Some((k, t)) if k == hit.kind => {
                        t.elapsed() >= Duration::from_secs(LIMIT_STUCK_SECS)
                    }
                    _ => {
                        seen.insert(pane.clone(), (hit.kind.to_string(), Instant::now()));
                        false
                    }
                }
            };
            if !stuck {
                continue;
            }
        } else {
            s.limit_seen.lock().await.remove(&pane);
        }
        {
            let mut map = s.limit_alert.lock().await;
            // Any recent alert suppresses, even on kind change: within one
            // continuous stall the first card (plus /read) already told
            // the owner everything; flips are scroll artifacts, not new
            // errors. The next genuine episode (after a confirmed clear)
            // or the 30-min re-remind still pages.
            if let Some((_, t)) = map.get(&pane)
                && t.elapsed() < Duration::from_secs(LIMIT_REMIND_SECS)
            {
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
