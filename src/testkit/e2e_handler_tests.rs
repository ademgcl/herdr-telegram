//! E2E handler cases: photos, refuses and topic routing — the surfaces
//! where a silent drop would be invisible to the owner if untested.

use super::Harness;
use crate::ui::{PHOTO_CMD_SKIPPED, PHOTO_HINT_GENERAL};

/// Photo with a command caption: the command runs as text and the
/// skipped image is VISIBLE (never a silent discard).
#[tokio::test]
async fn e2e_photo_with_command_caption_notes_the_skipped_image() {
    let h = Harness::start().await;
    h.map_topic();
    h.say_photo("/status", "fake-file-id").await;
    h.wait_for("skip note", || {
        h.sent_texts().iter().any(|t| t == PHOTO_CMD_SKIPPED)
    })
    .await;
    // No download attempt (commands serve imageless) and no prompt.
    assert_eq!(h.herdr.submit_count(), 0);
}

/// Photo in General (control plane) refuses visibly.
#[tokio::test]
async fn e2e_photo_in_general_refuses_visibly() {
    let h = Harness::start().await;
    // General: no message_thread_id (thread 1 is General in the wild).
    let mut general = h.message(serde_json::json!({
        "photo": [{"file_id": "f1", "width": 10, "height": 10, "file_size": 1}],
    }));
    general.as_object_mut().unwrap().remove("message_thread_id");
    h.update(general).await;
    h.wait_for("general photo hint", || {
        h.sent_texts().iter().any(|t| t == PHOTO_HINT_GENERAL)
    })
    .await;
    assert_eq!(h.herdr.submit_count(), 0);
}

/// Unknown command in a topic refuses; it never becomes a prompt.
#[tokio::test]
async fn e2e_unknown_command_never_becomes_a_prompt() {
    let h = Harness::start().await;
    h.map_topic();
    h.say("/definitely-not-a-command").await;
    h.wait_for("unknown-command reply", || {
        h.sent_texts()
            .iter()
            .any(|t| t.contains("command") || t.contains("Unknown"))
    })
    .await;
    assert_eq!(h.herdr.submit_count(), 0, "no prompt submitted");
}

/// A failed submit must be reported to the submitter, never silent.
#[tokio::test]
async fn e2e_failed_submit_reports_to_submitter() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.fail_next_submit("pane is not an agent");
    h.say("hello").await;
    h.wait_for("submit error card", || {
        h.sent_texts()
            .iter()
            .any(|t| t.contains("not an agent") || t.contains("⚠️"))
    })
    .await;
}
