//! Rate-limit stall scanner for job-less working panes. Split from
//! `reconcile` (300-line file limit).
use crate::{
    herdr::client::read_screen_adaptive,
    jobs::notices::{detect_limit, is_stuck_gated, limit_card_text},
    state::AppState,
};
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

/// Buzz once per limit episode on job-less working panes (prompt-owned
/// panes are the watcher's job — it replies in the prompt's own chat).
/// Once-per-episode survives noise: empty/outage reads preserve all
/// state (unknown ≠ clean), a banner must be absent for
/// [`LIMIT_CLEAR_MISSES`] consecutive clean reads to clear the episode,
/// gated (`provider`/`error`) banners must persist [`LIMIT_STUCK_SECS`]
/// before buzzing, and a recent alert suppresses re-pages regardless of
/// kind (scroll-order flips never spam; the 30-min remind still fires).
pub(crate) async fn scan_limits(s: &AppState) {
    // No nested locks (never hold status across jobs): snapshot working
    // panes, drop, then filter job-owned.
    let working: Vec<String> = {
        s.status
            .lock()
            .await
            .iter()
            .filter(|(_, st)| st.as_str() == "working")
            .map(|(p, _)| p.clone())
            .collect()
    };
    let owned: std::collections::HashSet<String> = s.jobs.lock().await.keys().cloned().collect();
    let panes: Vec<String> = working.into_iter().filter(|p| !owned.contains(p)).collect();
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
        if let Some(forum) = s.cfg.forum
            && let Some(thread) = s.topics.all_mappings().get(&pane).copied()
        {
            let mid = s
                .tg
                .send_msg_with_effect(
                    forum,
                    Some(thread),
                    &text,
                    None,
                    Some(crate::telegram::EFFECT_FIRE),
                )
                .await;
            if let Some(m) = mid {
                let _ = s.tg.set_reaction(forum, m, Some("❗")).await;
            }
            s.remember(forum, mid, &pane).await;
        } else {
            for id in &s.cfg.owners {
                let mid = s
                    .tg
                    .send_msg_with_effect(
                        *id,
                        None,
                        &text,
                        None,
                        Some(crate::telegram::EFFECT_FIRE),
                    )
                    .await;
                if let Some(m) = mid {
                    let _ = s.tg.set_reaction(*id, m, Some("❗")).await;
                }
                s.remember(*id, mid, &pane).await;
            }
        }
        println!("[alert] limit stall {pane}: {} ({})", hit.kind, hit.excerpt);
    }
}
