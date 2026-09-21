//! Mid-run limit/stall watch for prompt watchers. Split from `runner`
//! (300-line file limit): opencode retries internally while herdr keeps
//! reporting `working` — no settle, no notifier event, and live-message
//! edits never buzz. Scans the raw screen every tick and posts one NEW
//! (buzzing) card per episode.
use crate::{
    herdr::client::{get_agent, read_screen_for_limits},
    jobs::episode::BuzzEpisode,
    jobs::job::Job,
    jobs::notices::{detect_limit, limit_card_text},
    notifier::{
        LIMIT_REMIND_SECS,
        limit_decide::{alert_suppressed, send_cooled},
    },
    state::AppState,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

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
    // Reset owns topic lifecycle: skip the wide herdr read entirely —
    // cards posted mid-delete 429/fail, and the first post-reset tick
    // replays anything real anyway. Empty screen keeps streaming idle.
    if crate::handlers::reset::is_resetting() {
        return Vec::new();
    }
    let epoch_before = job.epoch.load(Ordering::Relaxed);
    let screen = read_screen_for_limits(&s.cfg.socket, pane).await;
    if screen.is_empty() {
        episode.note_empty();
        return screen;
    }
    let now = Instant::now();
    let detected = detect_limit(&screen);
    if detected.is_none() {
        // Clean read: on the CONFIRMED-clear transition (episode just
        // reset after sustained absence) also release the shared claim.
        // Duplicates-over-silence: one extra card on read disagreement
        // beats a genuine refire swallowed by the remind window.
        let was_open = !episode.is_fresh();
        episode.tick(None, now);
        if was_open && episode.is_fresh() {
            s.limit_alert.lock().await.remove(pane);
            s.limit_seen.lock().await.remove(pane);
            s.limit_miss.lock().await.remove(pane);
            s.limit_send_cool.lock().await.remove(pane);
        }
        return screen;
    }
    let Some(hit) = episode.tick(detected.as_ref(), now) else {
        return screen;
    };
    // Failed-send cooldown BEFORE claiming (episode kept unfired-ready):
    // a dead Telegram backs off instead of retrying every 5s tick.
    let cooled = s.limit_send_cool.lock().await.get(pane).copied();
    if send_cooled(cooled, now) {
        episode.unfire();
        return screen;
    }
    // Atomic check-and-claim under one guard (shared with the watchdog):
    // whoever claims first sends, the other stays silent — no double-page.
    // Same-kind only, so a provider blip never hides later quota. Pre-send,
    // but REMOVED on failure below, so drops never arm the 30-min suppress.
    let dup = {
        let mut map = s.limit_alert.lock().await;
        let prev = map.get(pane).map(|(k, t)| (k.clone(), *t));
        if alert_suppressed(
            hit.kind,
            prev.as_ref().map(|(k, t)| (k.as_str(), *t)),
            now,
            LIMIT_REMIND_SECS,
        ) {
            true
        } else {
            map.insert(pane.to_string(), (hit.kind.to_string(), now));
            false
        }
    };
    if dup {
        // Stay a hot standby: the winning path may still fail (releasing
        // its claim + cooling down) — unfired, we retry right after it.
        episode.unfire();
        return screen;
    }
    let (chat, th) = *job.dest.lock().await;
    // Epoch re-check (finalize parity): a submit landing during the
    // screen read above owns the pane — buzzing the old screen into the
    // new prompt's thread is stale. Release the claim, unfire, stay silent.
    if job.epoch.load(Ordering::Relaxed) != epoch_before {
        release_claim(s, pane, hit.kind, now).await;
        episode.unfire();
        return screen;
    }
    // Moved-on re-check (watchdog parity): a quit-to-shell landing
    // between the snapshot and the send must not page the stale ❗ into
    // the shelled topic. `None` (pruned/dead) is moved-on too — never
    // buzz into the unknown. Fresh read, not the watchdog-cached status
    // (up to 60s old): a PC-side quit inside the screen-read→send window
    // still shows the old status and pages the corpse thread.
    // Fail-closed: any non-agent verdict (shell, dead, unreadable)
    // releases the claim and stays silent — the next tick retries.
    let cur = s.status.lock().await.get(pane).cloned();
    if !matches!(cur.as_deref(), Some(v) if v != "shell") {
        release_claim(s, pane, hit.kind, now).await;
        episode.unfire();
        return screen;
    }
    if get_agent(&s.cfg.socket, pane).await.is_err() {
        // Fail-closed (send_keys parity): only a confirmed agent takes
        // the buzz — shell flips, dead panes and blips all stay silent
        // and retry next tick. list_panes is not consulted: a live shell
        // pane would read as "present" and page the corpse thread.
        release_claim(s, pane, hit.kind, now).await;
        episode.unfire();
        return screen;
    }
    // Remap-safe: a paced reset migrates the topic mid-run — buzzing
    // the corpse thread fails or lands in the deleted topic. Forum
    // dests follow the live mapping (the watchdog path already does);
    // DM dests never gain a thread. Unmapped forum dests retire: there
    // is nowhere to post, and falling back to the old thread retries
    // forever against a deleted topic (finalize_blocked mappable parity).
    let th = if s.cfg.forum == Some(chat) {
        match s.topics.storage.get_thread(pane) {
            Some(cur) => Some(cur),
            None => {
                release_claim(s, pane, hit.kind, now).await;
                episode.unfire();
                return screen;
            }
        }
    } else {
        th
    };
    let text = limit_card_text(pane, hit);
    let mid =
        s.tg.send_msg_with_effect(chat, th, &text, None, Some(crate::telegram::EFFECT_FIRE))
            .await;
    if let Some(m) = mid {
        // Post-send epoch guard (finalize parity): a submit landing
        // during the send owns the pane — the card buzzed the old
        // screen into the new prompt's thread. Delete best-effort,
        // never remember, release our claim.
        if job.epoch.load(Ordering::Relaxed) != epoch_before {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(crate::types::LIVE_RPC_TIMEOUT_SECS),
                s.tg.delete_msg(chat, m),
            )
            .await;
            release_claim(s, pane, hit.kind, now).await;
            episode.unfire();
            return screen;
        }
        let _ = s.tg.set_reaction(chat, m, Some("❗")).await;
        s.limit_send_cool.lock().await.remove(pane);
        s.remember(chat, mid, pane).await;
        println!("[prompt] limit alert {pane}: {}", hit.kind);
    } else {
        // Delivery failed: release the claim, cool down, and unfire so
        // the next tick retries with the stuck timer kept.
        release_claim(s, pane, hit.kind, now).await;
        s.limit_send_cool
            .lock()
            .await
            .insert(pane.to_string(), Instant::now());
        episode.unfire();
        eprintln!(
            "[prompt] limit alert {pane}: {} send FAILED — cooling down",
            hit.kind
        );
    }
    screen
}

/// Release our claim IFF still ours — a concurrent success that landed
/// after our claim must survive our failure path.
async fn release_claim(s: &AppState, pane: &str, kind: &str, claimed_at: Instant) {
    let mut map = s.limit_alert.lock().await;
    if matches!(map.get(pane), Some((k, t)) if k == kind && *t == claimed_at) {
        map.remove(pane);
    }
}
