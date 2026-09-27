//! E2E media + notifier cases: a photo must really download and reach
//! the agent as an absolute path, and an unsolicited agent finish must
//! buzz exactly once.
use super::Harness;

/// Photo in an agent topic: downloaded, written under the state dir and
/// attached to the prompt as an absolute path (agents resolve relative
/// to their own cwd).
#[tokio::test]
async fn e2e_photo_downloads_and_attaches_to_prompt() {
    let h = Harness::start().await;
    h.map_topic();
    // getFile carries no chat id, so count the delta, not the total.
    let getfile_before = h.tg.count_all("getFile");
    h.say_photo("why is this failing?", "file-abc").await;
    h.wait_for("submit with the image", || h.herdr.submit_count() >= 1)
        .await;
    let submitted = h.herdr.submitted().join("\n");
    assert!(
        submitted.contains("why is this failing?"),
        "caption kept: {submitted:?}"
    );
    assert!(
        submitted.contains("[attached image: /"),
        "absolute image path attached: {submitted:?}"
    );
    // The file really landed on disk (not just a marker in the text).
    let path = submitted
        .split("[attached image: ")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .expect("path segment")
        .trim()
        .to_string();
    assert!(
        std::path::Path::new(&path).exists(),
        "downloaded file exists: {path}"
    );
    // And the getFile + raw download really went through the API.
    assert_eq!(
        h.tg.count_all("getFile") - getfile_before,
        1,
        "exactly one getFile for one photo"
    );
}

/// Photo whose download fails: visible refusal, never a caption-only
/// prompt that silently drops the image.
#[tokio::test]
async fn e2e_failed_photo_download_refuses_visibly() {
    let h = Harness::start().await;
    h.map_topic();
    h.fault_any("getFile", "Bad Request: file is too big");
    h.say_photo("look at this", "file-abc").await;
    h.wait_for("fetch-failure notice", || {
        h.sent_texts()
            .iter()
            .any(|t| t.contains("couldn't fetch the image"))
    })
    .await;
    assert_eq!(
        h.herdr.submit_count(),
        0,
        "no half prompt without the image"
    );
}

/// Unsolicited finish: the agent goes idle with fresh output and nobody
/// prompted it — the watchdog path must buzz a card exactly once.
#[tokio::test]
async fn e2e_spontaneous_finish_posts_exactly_one_card() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_screen_owned(vec![
        "background job finished".into(),
        "all 42 tests passed".into(),
    ]);
    h.herdr.set_status("working");
    crate::notifier::status::observe_status(&h.s, &h.pane, "working", false, "e2e").await;
    h.herdr.set_status("idle");
    crate::notifier::status::observe_status(&h.s, &h.pane, "idle", false, "e2e").await;
    // The settle debounce is 15s — the wait budget must outlast it.
    h.wait_for_ticks("spontaneous card", 260, || async {
        h.sent_texts()
            .iter()
            .any(|t| t.contains("all 42 tests passed"))
    })
    .await;
    h.tick(6).await;
    let cards = h
        .sent_texts()
        .iter()
        .filter(|t| t.contains("all 42 tests passed"))
        .count();
    assert_eq!(cards, 1, "one spontaneous card, not a stream: {cards}");
}
