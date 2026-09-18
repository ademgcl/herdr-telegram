//! Tracked button surfaces per pane: which posted cards still carry
//! live answer buttons. Split from `dialog` (300-line file limit).
//!
//! Contracts (do not blur):
//! * [`track_card`] + [`repoint_card`]: same-chat replace — parallel
//!   posts (DM owners loop in refresh) must never strip each other.
//! * [`settle_card`]: a tap settled (chat, mid) as canonical — strip
//!   every other tracked card (sibling owners on a dead dialog).
//! * [`resolve_cards`]: dialog gone — strip all, consume, clear sig.
use crate::state::AppState;
use std::collections::HashMap;

/// One tracked button surface: (chat, message).
type Surface = (i64, i64);

/// Track a posted blocked card: one entry per chat, so a repost
/// replaces and a resolve strips every live surface exactly once.
/// Returns the replaced surface, if any — the caller strips it (a
/// turned-over dialog must not leave the dead dialog's buttons live).
/// Pure over the map for tests (lock lives with the caller).
pub(crate) fn track_card(
    map: &mut HashMap<String, Vec<Surface>>,
    pane: &str,
    chat: i64,
    mid: i64,
) -> Option<Surface> {
    let cards = map.entry(pane.to_string()).or_default();
    if let Some(slot) = cards.iter_mut().find(|(c, _)| *c == chat) {
        Some(std::mem::replace(slot, (chat, mid)))
    } else {
        cards.push((chat, mid));
        None
    }
}

/// Repoint tracking at a fresh post, stripping the replaced surface.
/// Same-chat only (see module docs). Lock is short; strip lock-free.
pub(crate) async fn repoint_card(s: &AppState, pane: &str, chat: i64, mid: i64) {
    let evicted = track_card(&mut *s.blocked_card.lock().await, pane, chat, mid);
    if let Some((c, m)) = evicted {
        s.tg.strip_buttons(c, m).await;
    }
}

/// Split tracked surfaces around a new live surface: the same-chat
/// slot is replaced, every other entry is stale (DM sibling owners
/// showing a dead dialog, earlier posts). Stale entries stay tracked
/// for resolve — buttonless after the strip, so re-strips are no-ops.
/// Pure for tests.
pub(crate) fn settle_select(
    old: Vec<Surface>,
    chat: i64,
    mid: i64,
) -> (Vec<Surface>, Vec<Surface>) {
    let mut keep = Vec::with_capacity(old.len() + 1);
    let mut stale = Vec::new();
    for (c, m) in old {
        if c == chat && m == mid {
            keep.push((c, m));
        } else {
            // Same-chat predecessor or sibling surface: strip now, but
            // keep siblings tracked so a later resolve still finds them.
            if c != chat {
                keep.push((c, m));
            }
            stale.push((c, m));
        }
    }
    if !keep.iter().any(|(c, m)| *c == chat && *m == mid) {
        keep.push((chat, mid));
    }
    (keep, stale)
}

/// Settle: (chat, mid) is now this pane's live surface — strip every
/// other tracked card. Same-surface calls only re-ensure tracking
/// (restart-emptied map). Lock is short; strips run lock-free.
pub(crate) async fn settle_card(s: &AppState, pane: &str, chat: i64, mid: i64) {
    let stale = {
        let mut map = s.blocked_card.lock().await;
        let old = map.remove(pane).unwrap_or_default();
        let (keep, stale) = settle_select(old, chat, mid);
        map.insert(pane.to_string(), keep);
        stale
    };
    for (c, m) in stale {
        s.tg.strip_buttons(c, m).await;
    }
}

/// PC-side (or quit-side) resolve: the dialog is gone — strip every
/// posted button surface so no doomed tap can fire, then drop the
/// signature. Consumes the tracked locations; a racing tap that owns
/// its card simply overwrites afterwards. Signature clears BEFORE the
/// strips: a concurrent poster re-stamps after us instead of being
/// wiped by a trailing remove. Locks are short and never held across
/// the strip calls.
pub(crate) async fn resolve_cards(s: &AppState, pane: &str) {
    let cards = s
        .blocked_card
        .lock()
        .await
        .remove(pane)
        .unwrap_or_default();
    s.blocked_sig.lock().await.remove(pane);
    for (chat, mid) in cards {
        s.tg.strip_buttons(chat, mid).await;
    }
}
