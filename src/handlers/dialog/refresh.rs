//! Content-addressed blocked-card refresh for status observations.
//! Split from `dialog` (300-line file limit): repeats of the same dialog
//! stay silent, a NEW dialog posts even with no status transition.
use super::{dialog_sig, is_blank_card, send_with};
use crate::{
    herdr::client::{get_agent, read_screen_visible},
    state::AppState,
};

/// True when this exact dialog already posted (content-addressed).
/// Single source for the pre-claim fast path + post-claim re-check.
async fn is_known_sig(s: &AppState, pane: &str, sig: &str) -> bool {
    s.blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == sig)
        .unwrap_or(false)
}

/// Content-addressed blocked post for status observations: repeats of
/// the same dialog stay silent, a NEW dialog posts even with no status
/// transition. Skips while a tap is in flight (it owns the update) and
/// when fresh herdr truth says the pane already moved on.
pub async fn refresh_blocked_card(s: &AppState, pane: &str) -> bool {
    // Taps own the update: a tap in flight wins over an observation.
    // Refresh-refresh single-flight happens at send time below (claim +
    // sig re-check), so a slow watchdog read never rejects user taps.
    // Self-healing peek: a stale corpse evicts instead of hiding cards.
    if s.block_held(pane).await {
        return false;
    }
    // Fail fast before slow RPCs: a missing mapping (human-deleted, mint
    // failure) stays silent until the next sync recreates the topic —
    // never burn herdr reads that can only return false below.
    if s.cfg.forum.is_some() && !s.topics.all_mappings().contains_key(pane) {
        return false;
    }
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status != "blocked" => {
            // Answered elsewhere (PC tap/dismiss, typed on the box):
            // the posted buttons must come off, not linger as bait.
            super::surfaces::resolve_cards(s, pane).await;
            return false;
        }
        // No agent here: shells never need blocked cards (a delayed
        // refresh after quit-to-shell would post a ghost). Fail-closed:
        // only a classified death consults the shell probe — a blip
        // (timeout) on a live blocked agent falls through to the screen
        // read below (empty on outage → silent, buttons untouched).
        Err(e) if crate::herdr::rpc::is_not_found(&e.to_string()) => {
            let shell = crate::herdr::client::list_panes(&s.cfg.socket)
                .await
                .map(|l| l.contains(&pane.to_string()))
                .unwrap_or(false);
            if shell {
                super::surfaces::resolve_cards(s, pane).await;
                return false;
            }
        }
        _ => {}
    }
    let screen = read_screen_visible(&s.cfg.socket, pane, 60).await;
    if screen.is_empty() || is_blank_card(&screen) {
        return false;
    }
    let sig = dialog_sig(&screen);
    if is_known_sig(s, pane, &sig).await {
        return false;
    }
    // Baseline follows delivery: a dropped card stays "new" (sig
    // unstamped on failure), so the next observation reposts from an
    // intact baseline instead of skewing the later idle delta.
    // Send-time single-flight: claim, then re-check the sig — a racing
    // refresh/tap that posted while we were reading wins, we stand down.
    // The claim is a map entry (non-blocking try-claim), never a mutex
    // held across the Telegram send below — exempt from the no-lock-
    // across-RPC rule by construction.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        return false;
    };
    if is_known_sig(s, pane, &sig).await {
        return false;
    }
    let posted = if let Some(forum) = s.cfg.forum {
        // Missing mapping (human-deleted, mint failure): never fall back
        // to the forum root (General) — one-topic-per-pane, stay silent
        // until the next sync recreates the topic (spontaneous parity).
        // Fresh read (not the pre-RPC snapshot): a remint racing the
        // slow reads must post into the NEW thread, never the dead one.
        let Some(thread) = s.topics.all_mappings().get(pane).copied() else {
            return false;
        };
        send_with(s, forum, Some(thread), pane, &screen).await
    } else {
        let mut posted = false;
        for id in &s.cfg.owners {
            if send_with(s, *id, None, pane, &screen).await {
                posted = true;
            }
        }
        posted
    };
    if posted {
        s.seen.lock().await.insert(pane.to_string(), screen);
        println!("[alert] blocked card {pane}");
    }
    posted
}
