//! Mid-run limit/stall watch for prompt watchers. Split from `runner`
//! (300-line file limit): opencode retries internally while herdr keeps
//! reporting `working` — no settle, no notifier event, and live-message
//! edits never buzz. Scans the raw screen every tick and posts one NEW
//! (buzzing) card per episode.
use crate::{
    herdr::client::read_screen_adaptive,
    jobs::episode::BuzzEpisode,
    jobs::job::Job,
    jobs::notices::{detect_limit, limit_card_text},
    notifier::LIMIT_REMIND_SECS,
    state::AppState,
};
use std::sync::Arc;
use tokio::time::Duration;

/// Scan the raw screen for a stall banner and buzz once per episode.
/// Runs on the raw screen, before chrome filtering, and outside the
/// edit cooldown so stalls surface even when nothing streams.
/// Empty read = outage/unknown: preserve the episode (never reset dedup
/// on a failed read) and retry next tick. Returns the screen for the
/// streaming stage.
pub(crate) async fn watch_stall(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    episode: &mut BuzzEpisode,
) -> Vec<String> {
    let screen = read_screen_adaptive(&s.cfg.socket, pane).await;
    if screen.is_empty() {
        episode.note_empty();
    } else if let Some(hit) =
        episode.tick(detect_limit(&screen).as_ref(), std::time::Instant::now())
    {
        // Cross-path dedup: the watchdog shares `limit_alert` — if it
        // (or a prior watcher) already buzzed this kind within the
        // remind window, stay silent so handoffs never double-page.
        // Check + claim hold one lock guard (atomic): concurrent
        // watchdog ticks then see the claim and stay silent too.
        let dup = {
            let mut map = s.limit_alert.lock().await;
            match map.get(pane) {
                Some((k, t))
                    if k == hit.kind && t.elapsed() < Duration::from_secs(LIMIT_REMIND_SECS) =>
                {
                    true
                }
                _ => {
                    map.insert(
                        pane.to_string(),
                        (hit.kind.to_string(), std::time::Instant::now()),
                    );
                    false
                }
            }
        };
        if !dup {
            let (chat, th) = *job.dest.lock().await;
            let text = limit_card_text(pane, hit);
            let mid = s
                .tg
                .send_msg_with_effect(chat, th, &text, None, Some(crate::telegram::EFFECT_FIRE))
                .await;
            if let Some(m) = mid {
                let _ = s.tg.set_reaction(chat, m, Some("❗")).await;
            }
            s.remember(chat, mid, pane).await;
            println!("[prompt] limit alert {pane}: {}", hit.kind);
        }
    }
    screen
}
