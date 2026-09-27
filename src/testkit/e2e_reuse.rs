//! Cross-turn reuse: a turn's output must never surface in a later
//! turn's working message. Split from `e2e_delivery` (300-line file
//! limit).
use super::Harness;

/// A turn's output must never reappear in a LATER turn's working message.
///
/// The reported symptom: the first message was perfect, the second's
/// `💭 thinking…` message showed the first turn's reply under the header.
/// A delivered turn's accumulator is spent — the epoch check that
/// normally drops it only runs on the next tick, so between the final
/// landing and that tick the working message could still render it.
#[tokio::test]
async fn e2e_delivered_reply_never_reappears_in_a_later_transient() {
    let h = Harness::start().await;
    h.map_topic();
    let delivered = "the delivered first turn answer";

    h.run_turn("first", &[delivered]).await;

    // Second turn: its own instant, and the working message must never
    // carry the first turn's delivered text.
    h.say("second").await;
    h.wait_for("second submit", || h.herdr.submit_count() >= 2)
        .await;
    let second_instant = h
        .sends()
        .iter()
        .filter(|c| c.text().starts_with(crate::jobs::progress::THINKING))
        .map(|c| c.sent_id())
        .next_back()
        .expect("second turn posted its own working message");
    h.set_reply_text("the second turn answer");
    h.wait_for("second final", || {
        h.sends()
            .iter()
            .any(|c| c.text().contains("the second turn answer"))
    })
    .await;

    // Scoped to the SECOND turn's working message: turn one's own final
    // legitimately contains its reply, so only turn two's message is the
    // one that must be clean.
    let mut all = h.sends();
    all.extend(h.edits());
    let seen_in_turn_two: Vec<String> = all
        .iter()
        .filter(|c| c.message_id() == second_instant)
        .map(|c| c.text().to_string())
        .collect();
    assert!(
        !seen_in_turn_two.iter().any(|t| t.contains(delivered)),
        "turn one's delivered reply reappeared in turn two's working message: {seen_in_turn_two:?}"
    );
}
