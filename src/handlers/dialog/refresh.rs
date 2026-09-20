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

/// Post-read verdict for the blocked-card gate (single source for the
/// pre + post slow-read checks): a resume owns the pane (resolve, no
/// post), a classified-dead shell resolves too, blips fall through so an
/// outage never bricks real dialogs. Pure for tests.
#[derive(Debug, PartialEq)]
pub(crate) enum RefreshGate {
    Proceed,
    Resolve,
}

pub(crate) fn post_read_gate(status: Result<&str, &str>, is_shell: bool) -> RefreshGate {
    match status {
        Ok("blocked") => RefreshGate::Proceed,
        Ok(_) => RefreshGate::Resolve,
        Err(m) if crate::herdr::rpc::is_not_found(m) && is_shell => RefreshGate::Resolve,
        Err(_) => RefreshGate::Proceed,
    }
}

/// Single RPC gate for both pre + post slow-read checks: classifies via
/// post_read_gate (pinned). Shell probe runs only on classified death.
async fn refresh_gate(s: &AppState, pane: &str) -> RefreshGate {
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => post_read_gate(Ok(a.status.as_str()), false),
        Err(e) => {
            let msg = e.to_string();
            if crate::herdr::rpc::is_not_found(&msg) {
                let shell = crate::herdr::client::list_panes(&s.cfg.socket)
                    .await
                    .map(|l| l.contains(&pane.to_string()))
                    .unwrap_or(false);
                post_read_gate(Err(msg.as_str()), shell)
            } else {
                post_read_gate(Err(msg.as_str()), false)
            }
        }
    }
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
    // A prompt/follow watcher owns the pane: its settle path posts the
    // blocked card itself (finalize_blocked, claim + double sig-check),
    // so a lag-`blocked` heal must not ghost-post working prose with
    // live buttons beside the coming final. Mirrors observe_status's
    // job_owned early return and settle_check's jobs guards.
    if s.job_live(pane).await {
        return false;
    }
    // Fail fast before slow RPCs: a missing mapping (human-deleted, mint
    // failure) stays silent until the next sync recreates the topic —
    // never burn herdr reads that can only return false below.
    if s.cfg.forum.is_some() && !s.topics.all_mappings().contains_key(pane) {
        return false;
    }
    // No agent here: shells never need blocked cards (a delayed refresh
    // after quit-to-shell would post a ghost). Answered elsewhere also
    // resolves (buttons must not linger as bait).
    if refresh_gate(s, pane).await == RefreshGate::Resolve {
        super::surfaces::resolve_cards(s, pane).await;
        return false;
    }
    let screen = read_screen_visible(&s.cfg.socket, pane, super::DIALOG_READ_LINES).await;
    if screen.is_empty() || is_blank_card(&screen) {
        return false;
    }
    // Post-read re-validation: a resume during the slow read owns the
    // pane now — never post a blocked card with live buttons into live
    // work. Blips fall through (outage must not brick real dialogs).
    // A watcher that started during the read owns the card too (pre-read
    // jobs gate above, re-checked: same reason, second window).
    if refresh_gate(s, pane).await == RefreshGate::Resolve {
        super::surfaces::resolve_cards(s, pane).await;
        return false;
    }
    if s.job_live(pane).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_post_read_gate_resume_vs_blip() {
        // Live blocked proceeds; a resume during the slow read resolves.
        assert_eq!(post_read_gate(Ok("blocked"), false), RefreshGate::Proceed);
        assert_eq!(post_read_gate(Ok("working"), false), RefreshGate::Resolve);
        assert_eq!(post_read_gate(Ok("idle"), true), RefreshGate::Resolve);
        // Classified-dead shell resolves (no ghost card); the same death
        // with no shell falls through (pre-existing shell probe decides).
        assert_eq!(
            post_read_gate(Err("agent_not_found"), true),
            RefreshGate::Resolve
        );
        assert_eq!(
            post_read_gate(Err("agent_not_found"), false),
            RefreshGate::Proceed
        );
        // Blips fall through — an outage must not brick real dialogs.
        assert_eq!(
            post_read_gate(Err("herdr timed out"), false),
            RefreshGate::Proceed
        );
        assert_eq!(
            post_read_gate(Err("herdr timed out"), true),
            RefreshGate::Proceed
        );
    }
}
