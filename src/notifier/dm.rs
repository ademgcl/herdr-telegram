//! Direct Message alert delivery (split from status.rs to respect the 300-line limit).
use crate::{
    jobs::segment::final_block,
    jobs::stream::{delta, is_stale_body, join_trimmed},
    notifier::spontaneous::post_spontaneous_card,
    state::AppState,
};
use std::time::{Duration, Instant};

use super::POST_PROMPT_QUIET_SECS;

/// DM mode has no topics — legacy immediate pushes. The baseline is
/// consumed only on delivery so an outage replays the delta instead
/// of eating it. Fresh work recomputes the delta vs CURRENT seen just
/// before post (a final retiring during the screen RPC anchors seen —
/// the pre-RPC body would else re-post the final's duplicate).
pub(crate) async fn push_dm_alert(
    s: &AppState,
    pane: &str,
    kind: &str,
    raw_space: &str,
    new_status: &str,
    screen: &[String],
    observed_at: Instant,
) {
    // Fresh re-check + re-base (no extra RPC, just the lock).
    if s.job_live(pane).await {
        return;
    }
    let fresh_base = s.seen.lock().await.get(pane).cloned().unwrap_or_default();
    let fresh_source: Vec<String> = if fresh_base.is_empty() {
        screen.to_vec()
    } else {
        delta(screen, &fresh_base).to_vec()
    };
    let fresh_body = join_trimmed(&final_block(&fresh_source, ""));
    // Stale re-extraction (forum settle_check parity): the whole body
    // already sits in the anchored baseline — fall through to the quiet
    // anchor below instead of re-buzzing a delivered final. Blocked
    // exempt: questions re-buzz by design until answered.
    let stale = new_status != "blocked" && is_stale_body(&fresh_body, &fresh_base);
    if stale {
        println!("[alert] suppressed duplicate final for {pane}");
    }
    if !fresh_body.is_empty() && !stale {
        // Some(observed_at): a final retiring during the screen RPC
        // stamps last_done after observed_at → inside-post suppresses
        // the stale duplicate. Next tick's observed_at is after the
        // final, so genuinely new work still posts.
        if post_spontaneous_card(
            s,
            pane,
            kind,
            raw_space,
            new_status,
            &fresh_body,
            Some(observed_at),
            true,
        )
        .await
        {
            s.seen
                .lock()
                .await
                .insert(pane.to_string(), screen.to_vec());
        }
        return;
    }
    // Empty (duplicate of a just-posted final, or genuinely empty):
    // recent final → consume + quiet; anything else stays silent too —
    // buzzing on empty/unreadable settles violates §3 (silent when empty
    // or moved-on) and fail-closed (no buzz on ambiguous reads). The
    // forum path anchors silently the same way.
    if let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
        && !screen.is_empty()
    {
        s.seen
            .lock()
            .await
            .insert(pane.to_string(), screen.to_vec());
    } else if !screen.is_empty() {
        // No recent final and a readable screen: anchor silently so the
        // same delta doesn't re-RPC every tick (forum stray parity).
        // Empty screens never anchor (outage must not wipe the baseline).
        s.seen
            .lock()
            .await
            .insert(pane.to_string(), screen.to_vec());
    }
}
