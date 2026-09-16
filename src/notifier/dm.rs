//! Direct Message alert delivery (split from status.rs to respect the 300-line limit).
use crate::{
    jobs::segment::final_block,
    jobs::stream::{delta, join_trimmed},
    notifier::cards::post_spontaneous_card,
    state::AppState,
    ui::{btn, emoji},
};
use serde_json::json;
use std::time::{Duration, Instant};

const POST_PROMPT_QUIET_SECS: u64 = 45;

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
    space_label: &str,
    title: &str,
    new_status: &str,
    screen: &[String],
    observed_at: Instant,
) {
    // Fresh re-check + re-base (no extra RPC, just the lock).
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }
    let fresh_base = s.seen.lock().await.get(pane).cloned().unwrap_or_default();
    let fresh_source: Vec<String> = if fresh_base.is_empty() {
        screen.to_vec()
    } else {
        delta(screen, &fresh_base).to_vec()
    };
    let fresh_body = join_trimmed(&final_block(&fresh_source, ""));
    if !fresh_body.is_empty() {
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
        )
        .await
        {
            s.seen.lock().await.insert(pane.to_string(), screen.to_vec());
        }
        return;
    }
    // Empty (duplicate of a just-posted final, or genuinely empty):
    // recent final → consume + quiet; else fall through to the hint.
    if let Some(t) = s.last_done.lock().await.get(pane)
        && t.elapsed() < Duration::from_secs(POST_PROMPT_QUIET_SECS)
    {
        s.seen.lock().await.insert(pane.to_string(), screen.to_vec());
        return;
    }
    let hint = "";
    let verb = if new_status == "idle" {
        "ready"
    } else {
        new_status
    };
    let mut text = format!("{} {}: {kind} @ {space_label}", emoji(new_status), verb);
    if !title.is_empty() {
        let short: String = title.chars().take(60).collect();
        text.push_str(&format!("\n{short}"));
    }
    text.push_str(hint);
    let mut delivered = true;
    for id in &s.cfg.owners {
        let mid = s
            .tg
            .send_msg(
                *id,
                None,
                &text,
                Some(json!([[btn("show output", &format!("o:{pane}"))]])),
            )
            .await;
        if let Some(m) = mid {
            if new_status == "done" {
                let _ = s.tg.set_reaction(*id, m, Some("✅")).await;
            }
        } else {
            delivered = false;
        }
        s.remember(*id, mid, pane).await;
    }
    // Baseline advances only on delivery: an outage replays the
    // delta instead of eating it. Stamp last_done like a posted
    // card so the next settle in the quiet window stays silent.
    if delivered {
        s.seen.lock().await.insert(pane.to_string(), screen.to_vec());
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), Instant::now());
    }
}
