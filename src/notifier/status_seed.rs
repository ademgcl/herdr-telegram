//! Boot-seed blocked card (split from `status`, 300-line file limit).
//! An already-blocked pane genuinely needs input NOW (its question was
//! missed while the bot was down), so the seed posts the answer card
//! instead of staying mute until the next transition. Content-addressed
//! like every other poster: a watcher's blocked settle cards the same
//! dialog and observes with `silent`, so without this gate the seed
//! posted a SECOND copy of one question (the "everything arrives
//! twice" report).
use crate::{
    handlers::dialog::{dialog_sig, send_blocked_card, sig_matches},
    herdr::client::read_screen_visible,
    state::AppState,
};

pub(crate) async fn seed_blocked_card(
    s: &AppState,
    pane: &str,
    old: Option<&str>,
    new_status: &str,
) {
    if old.is_some() || new_status != "blocked" {
        return;
    }
    println!("[alert] seed found {pane} blocked — posting answer card");
    let screen = read_screen_visible(&s.cfg.socket, pane, 80).await;
    let sig = dialog_sig(&screen);
    let stamped = s.blocked_sig.lock().await.get(pane).cloned();
    if sig_matches(stamped.as_ref(), &sig) {
        println!("[alert] seed {pane}: dialog already carded, staying silent");
        return;
    }
    // Baseline follows delivery: a dropped seed card must stay "new" so
    // the next observation posts it.
    let posted = if let Some(forum) = s.cfg.forum {
        // One-topic-per-pane (refresh parity): a missing mapping never
        // falls back to the forum root — stay silent until the next sync
        // recreates the topic. Single read: bind once, no contains-then-
        // get skew across lock releases.
        let thread = s.topics.all_mappings().get(pane).copied();
        if thread.is_none() {
            return;
        }
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
