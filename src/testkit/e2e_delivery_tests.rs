//! E2E delivery cases: what the owner actually receives for a turn.
//! Every case drives the production router + watcher against a fake
//! Telegram API, so the assertions are on real sends/edits/deletes.

use super::Harness;
use crate::testkit::tg_fake::Call;

fn edits(h: &Harness) -> Vec<Call> {
    h.edits()
}
fn sends(h: &Harness) -> Vec<Call> {
    h.sends()
}

/// Mid of the silent working message. Content-addressed, never
/// `sends[0]`: the 📌 identity card is a legitimate earlier send, and an
/// index-based pick silently starts asserting about the pin.
fn instant_mid(h: &Harness) -> i64 {
    sends(h)
        .iter()
        .find(|c| c.text().contains(crate::jobs::progress::THINKING))
        .unwrap_or_else(|| panic!("no instant message in {:#?}", h.sent_texts()))
        .sent_id()
}

/// Happy path: instant message first, final as a NEW message, and the
/// working message deleted when auto-remove is on.
#[tokio::test]
async fn e2e_turn_posts_instant_then_final_and_retires_transient() {
    let h = Harness::start().await;
    h.map_topic();
    h.s.set_transient_remove(true).await;
    h.run_turn("hello", &["hi there"]).await;

    let sent = sends(&h);
    // 1. the instant working message precedes the final
    let instant = instant_mid(&h);
    let instant_at = sent
        .iter()
        .position(|c| c.sent_id() == instant)
        .unwrap_or_else(|| panic!("instant not among the sends"));
    assert!(
        instant_at + 1 < sent.len(),
        "instant must precede the final"
    );
    // 2. the final is a NEW message carrying the answer
    let final_text = sent.last().unwrap().text();
    assert!(final_text.contains("hi there"), "got {final_text:?}");
    assert_ne!(
        instant,
        sent.last().unwrap().sent_id(),
        "final must be its own message"
    );
    // 3. transient retired exactly once (the delete lands after the
    // final — the retire is gated on the final's delivery).
    h.wait_for("transient retired", || h.sent_count("deleteMessage") >= 1)
        .await;
    assert_eq!(h.sent_count("deleteMessage"), 1, "one delete only");
}

/// Auto-remove off (the default): the working message stays as history
/// and is reused by the next turn instead of spamming new ones.
#[tokio::test]
async fn e2e_transient_kept_by_default_and_reused_next_turn() {
    let h = Harness::start().await;
    h.map_topic();
    assert!(!h.s.transient_remove());
    h.run_turn("first", &["one"]).await;
    assert_eq!(h.sent_count("deleteMessage"), 0, "kept, never deleted");
    let first_instant = instant_mid(&h);

    h.run_turn("second", &["two"]).await;
    let second_final = sends(&h)
        .iter()
        .find(|c| c.text().contains("two"))
        .expect("second final")
        .sent_id();
    assert_ne!(second_final, first_instant, "final is still a new message");
    let reused = edits(&h).iter().any(|c| c.message_id() == first_instant);
    assert!(reused, "second turn must reuse the kept transient");
    // Exactly two working messages existed (one per turn), never three.
    assert!(
        sends(&h)
            .iter()
            .filter(|c| c.text().contains("thinking"))
            .count()
            <= 1,
        "no duplicate instant messages: {:?}",
        h.sent_texts()
    );
}

/// Dead transient (the user deleted it): the next turn must detect and
/// re-post, never run instant-less. This is the reported bug.
#[tokio::test]
async fn e2e_dead_transient_is_detected_and_reposted() {
    let h = Harness::start().await;
    h.map_topic();
    h.run_turn("first", &["one"]).await;
    // Telegram says the kept message no longer exists.
    h.fault("editMessageText", "Bad Request: MESSAGE_ID_INVALID");
    h.herdr.set_status("working");
    h.say("second").await;
    h.wait_for("re-post after dead slot", || {
        h.sends()
            .iter()
            .filter(|c| c.text().contains("thinking"))
            .count()
            >= 2
    })
    .await;
}

/// No-op edit (already the placeholder) must converge, not re-post and
/// not error — a bogus failure here would spam every turn.
#[tokio::test]
async fn e2e_not_modified_probe_converges_without_repost() {
    let h = Harness::start().await;
    h.map_topic();
    h.run_turn("first", &["one"]).await;
    // Count working messages, not all sends: the second turn's FINAL
    // is a legitimate new message and must not be confused with a
    // re-posted placeholder.
    let placeholders = |h: &Harness| -> usize {
        h.sends()
            .iter()
            .filter(|c| c.text().contains(crate::jobs::progress::THINKING))
            .count()
    };
    let before = placeholders(&h);
    h.fault("editMessageText", "Bad Request: message is not modified");
    h.herdr.set_status("working");
    h.say("second").await;
    h.wait_for("second submit", || h.herdr.submit_count() >= 2)
        .await;
    h.wait_for_final_after(0).await;
    // No fresh instant post: the slot was alive after all.
    assert_eq!(placeholders(&h), before, "converged probe re-posted");
}

/// A 429 on the instant post must back off, never duplicate, and still
/// deliver the final.
#[tokio::test]
async fn e2e_flood_on_instant_backs_off_without_duplicate() {
    let h = Harness::start().await;
    h.map_topic();
    h.fault("sendMessage", "Too Many Requests: retry after 1");
    h.run_turn("hello", &["hi"]).await;
    // Exactly one instant + one final: the flooded attempt banked and
    // the retry produced no twin.
    let texts = h.sent_texts();
    assert_eq!(
        texts.iter().filter(|t| t.contains("thinking")).count(),
        1,
        "no duplicate instant: {texts:?}"
    );
    assert!(texts.iter().any(|t| t.contains("hi")));
}

/// Reply-latency contract, and the safety half of it.
///
/// A final must NOT post while the agent is still streaming (that is a
/// partial answer, then a second "final" — the double-post bug the old
/// fixed 5s same-kind wait existed to prevent), and it must post soon
/// after the pane goes quiet. The gate is output-quiet, not wall-clock:
/// fresh bytes re-stamp the arm, so a still-streaming turn cannot
/// commit no matter how long it runs, and a finished one commits in
/// ~2s of quiet instead of waiting out 5s.
#[tokio::test]
async fn e2e_final_waits_for_quiet_then_lands_promptly() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_status("working");
    h.herdr.set_screen(&["> hi", "Thinking…"]);
    h.say("hi").await;
    h.wait_for("submit", || h.herdr.submit_count() >= 1).await;
    h.wait_for("instant", || {
        h.sends()
            .iter()
            .any(|c| c.text().contains(crate::jobs::progress::THINKING))
    })
    .await;

    // Settled status, but the pane KEEPS producing output: past the old
    // gate's own window, no final may exist yet.
    h.herdr.set_status("idle");
    for i in 0..4 {
        h.herdr.set_screen(&[
            "> hi",
            "Thinking…",
            &format!("chunk {i}"),
            " ⬝ esc interrupt   145.6K (14%)  ctrl+p commands",
        ]);
        h.tick(1).await;
    }
    assert!(
        h.sends()
            .iter()
            .all(|c| c.text().contains(crate::jobs::progress::THINKING) || c.text().contains("📌")),
        "a final posted while output was still flowing: {:#?}",
        h.sent_texts()
    );

    // Output stops: the final must now land, and quickly.
    let quiet_at = std::time::Instant::now();
    h.herdr.set_screen(&[
        "> hi",
        "all done here",
        " ⬝ esc interrupt   145.6K (14%)  ctrl+p commands",
    ]);
    h.wait_for("final after quiet", || {
        h.sends().iter().any(|c| c.text().contains("all done here"))
    })
    .await;
    assert!(
        quiet_at.elapsed() < std::time::Duration::from_millis(5_000),
        "final took {:?} after the pane went quiet",
        quiet_at.elapsed()
    );
}

/// Pickup contract: the silent working message must land on the user's
/// Telegram *before* the (slow) submit RPC, not after it. The submit
/// is the one 30s-budgeted call in the handler and it runs inline in the
/// poll loop, so anything ordered after it is invisible latency — the
/// "my message arrived late" report. A baseline screen read hoisted in
/// front of the placeholder would break this.
#[tokio::test]
async fn e2e_instant_lands_before_the_submit_rpc() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_status("working");
    h.herdr.set_screen(&["> hi", "Thinking…"]);
    let sent_at = std::time::Instant::now();
    h.say("hi").await;
    h.wait_for("instant", || {
        h.sends()
            .iter()
            .any(|c| c.text().contains(crate::jobs::progress::THINKING))
    })
    .await;
    let instant_ms = sent_at.elapsed().as_millis();
    assert!(
        h.herdr.submit_count() >= 1,
        "the turn never reached the agent"
    );
    assert!(
        instant_ms < 1_500,
        "instant message took {instant_ms}ms to land"
    );
}
