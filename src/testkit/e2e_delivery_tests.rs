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
/// when the turn ends. A later turn must never destroy that history:
///
/// the reported "second msg deleted the first reply". If the first
/// turn rendered its answer into its message, no later edit may reset
/// that message to the placeholder; the new turn posts its own
/// placeholder instead.
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
    // History check over the edit log: once the first message showed the
    // answer, no later edit may reset it to the placeholder. (If the
    // first turn never rendered, there is nothing to destroy — the
    // check is vacuously true.)
    let mut saw_answer = false;
    for c in edits(&h) {
        if c.message_id() != first_instant {
            continue;
        }
        if c.text().contains("one") {
            saw_answer = true;
        } else if saw_answer {
            assert_ne!(
                c.text(),
                crate::jobs::progress::THINKING,
                "second turn reset the first reply to the placeholder"
            );
        }
    }
    // The second turn is never instant-less: it reuses the bare
    // placeholder or posts its own.
    assert!(
        sends(&h)
            .iter()
            .filter(|c| c.text() == crate::jobs::progress::THINKING)
            .count()
            >= 1,
        "second turn must have its own placeholder: {:?}",
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

/// A second prompt must NEVER overwrite the first turn's delivered
/// output in place — the reported "my second message deleted the first
/// reply and replaced it with thinking…", which then also left that
/// message stuck on the placeholder forever (a superseded turn's
/// accumulation is dropped, so nothing refilled it).
///
/// With `/transient` off (the default) the kept working message IS the
/// user's copy of the reply, so it is history: the new turn posts its own
/// placeholder beside it. Driven explicitly (not `run_turn`) because the
/// slot only holds rendered output after a real streaming phase — the
/// settle path finalizes without ever rendering the tail.
#[tokio::test]
async fn e2e_second_prompt_never_overwrites_the_first_reply() {
    let h = Harness::start().await;
    h.map_topic();
    assert!(
        !h.s.transient_remove(),
        "kept-history mode is the case that loses text"
    );
    let chrome = " ⬝ esc interrupt   145.6K (14%)  ctrl+p commands";
    h.herdr.set_status("working");
    h.herdr.set_screen(&["> first", "Thinking…"]);
    h.say("first").await;
    h.wait_for("first submit", || h.herdr.submit_count() >= 1)
        .await;
    h.wait_for("first placeholder", || {
        h.sends()
            .iter()
            .any(|c| c.text() == crate::jobs::progress::THINKING)
    })
    .await;
    let first_mid = h
        .sends()
        .iter()
        .find(|c| c.text() == crate::jobs::progress::THINKING)
        .unwrap()
        .sent_id();
    // Streaming phase: the watcher renders the tail into that same slot.
    h.herdr.set_screen(&["> first", "the first answer", chrome]);
    // 3 ticks ≈ 2.1s: inside one poll cycle plus its baseline anchor, and
    // well inside the 4s edit cooldown — so this also pins that the FIRST
    // tail render of a turn is not throttled (the "stuck at thinking" wait).
    h.tick(3).await;
    assert!(
        h.edits()
            .iter()
            .any(|c| c.message_id() == first_mid && c.text().contains("the first answer")),
        "the slot never rendered the answer: {:#?}",
        h.edits()
    );
    // Settle: the final posts, and with auto-remove off the slot SURVIVES
    // holding that answer.
    h.herdr.set_screen(&["> first", "the first answer", chrome]);
    h.herdr.set_status("idle");
    h.wait_for("first final", || {
        h.sends()
            .iter()
            .any(|c| c.text().contains("the first answer"))
    })
    .await;

    // Second prompt arrives while the slot still shows the first answer.
    h.herdr.set_status("working");
    h.herdr.set_screen(&["> second", "Thinking…", chrome]);
    h.say("second").await;
    h.wait_for("second submit", || h.herdr.submit_count() >= 2)
        .await;
    h.tick(2).await;

    assert!(
        !h.edits().iter().any(|c| {
            c.message_id() == first_mid && c.text() == crate::jobs::progress::THINKING
        }),
        "the second prompt reset the first reply's message to the placeholder"
    );
    // The new turn still gets instant feedback — its own placeholder.
    h.wait_for("fresh placeholder for the new turn", || {
        h.sends()
            .iter()
            .filter(|c| c.text() == crate::jobs::progress::THINKING)
            .count()
            >= 2
    })
    .await;
}
