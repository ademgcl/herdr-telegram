//! E2E state cases: typing ownership, pass-through delivery, cancel,
//! and the duplicate-final suppression — the "state problem" class the
//! owner reported (indicator stuck, ghost copies, out-of-sync).

use super::Harness;

/// Pass-through: two messages in a row both reach the agent, with no
/// queue ack and nothing stranded.
#[tokio::test]
async fn e2e_two_prompts_both_delivered_no_queue() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_status("working");
    h.say("first").await;
    h.say("second").await;
    h.wait_for("both submits", || h.herdr.submit_count() >= 2)
        .await;
    let submits = h.herdr.submitted();
    assert_eq!(submits.len(), 2, "both prompts delivered");
    assert!(submits.iter().any(|s| s == "first"));
    assert!(submits.iter().any(|s| s == "second"));
    // No queue vocabulary anywhere in what the user saw.
    assert!(
        !h.sent_texts().iter().any(|t| t.contains("queued")),
        "no queue acks: {:?}",
        h.sent_texts()
    );
}

/// Typing starts with the turn and stops when the pane goes idle with
/// no intent left — the stuck-indicator report.
#[tokio::test]
async fn e2e_typing_stops_after_turn_settles() {
    let h = Harness::start().await;
    h.map_topic();
    h.run_turn("hello", &["done"]).await;
    h.wait_for_async("typing stopped", || async { h.typing_tasks().await == 0 })
        .await;
}

/// Typing survives a mid-turn cancel only if something still owns the
/// pane; a cancel with nothing pending must leave no task behind.
#[tokio::test]
async fn e2e_typing_no_leak_after_cancel() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_status("working");
    h.say("hello").await;
    h.wait_for("submit", || h.herdr.submit_count() >= 1).await;
    h.say("/cancel").await;
    h.wait_for_async("typing task gone", || async { h.typing_tasks().await == 0 })
        .await;
    // The cancel is reported; the abandoned turn never posts a final.
    assert!(
        h.sent_texts().iter().any(|t| t.contains("cancelled")),
        "cancel reported: {:?}",
        h.sent_texts()
    );
}

/// A final that already landed must not be re-buzzed by the settle
/// notifier (the "everything posts twice" report).
#[tokio::test]
async fn e2e_final_is_not_reposted_as_spontaneous() {
    let h = Harness::start().await;
    h.map_topic();
    h.run_turn("hello", &["the one answer"]).await;
    h.tick(6).await;
    let answers: Vec<String> = h
        .sent_texts()
        .into_iter()
        .filter(|t| t.contains("the one answer"))
        .collect();
    assert_eq!(
        answers.len(),
        1,
        "answer delivered exactly once, got {answers:?}"
    );
}

/// Non-owner traffic must never reach an agent.
#[tokio::test]
async fn e2e_non_owner_message_never_submits() {
    let h = Harness::start().await;
    h.map_topic();
    let mut stranger = h.message(serde_json::json!({"text": "let me in"}));
    stranger["from"] = serde_json::json!({"id": 999});
    h.update(stranger).await;
    h.tick(2).await;
    assert_eq!(h.herdr.submit_count(), 0, "stranger never submitted");
    assert!(h.sent_texts().is_empty(), "and got no reply");
}
