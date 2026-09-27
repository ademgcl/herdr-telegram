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
    h.s.set_transient_on(true).await; // asserts on the working message
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

/// With working messages off, `/read` is how the user sees the pane.
///
/// It must keep working: `/read` reads the agent's screen straight from
/// herdr and has never touched the transient slot, so switching the
/// default off must not take the only remaining way to read a reply with
/// it. Pinned here because the flag change is exactly the kind of edit
/// that could quietly couple the two.
#[tokio::test]
async fn e2e_read_still_works_with_transient_off() {
    let h = Harness::start().await;
    h.map_topic();
    assert!(!h.s.transient_on(), "working messages are off by default");

    let marker = "a distinctive line only the pane knows";
    h.herdr.set_screen(&["> a prompt", marker]);
    h.say("/read").await;
    h.wait_for("/read replied with the pane", || {
        h.sent_texts().iter().any(|t| t.contains(marker))
    })
    .await;
}

/// A closed topic must be DELETED, not left greyed out in the forum.
///
/// Telegram refuses to delete an open topic, so the close path closes and
/// then deletes. Before this, every finished agent left a dead topic
/// holding its slot in the list forever.
#[tokio::test]
async fn e2e_closed_topic_is_deleted_not_left_behind() {
    let h = Harness::start().await;
    h.map_topic();
    let thread = h.thread;
    h.s.topics.close_topic_for_thread(&h.pane, thread).await;
    assert_eq!(h.sent_count("closeForumTopic"), 1, "topic closed");
    assert_eq!(
        h.sent_count("deleteForumTopic"),
        1,
        "a closed topic must be deleted, not left in the forum"
    );
}
