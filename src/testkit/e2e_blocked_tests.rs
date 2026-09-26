//! E2E blocked-dialog flow: the reason this bot exists. A blocked agent
//! must post a question card with working buttons, and a tap must
//! actually answer the agent (keys reach the pane) — not just look
//! right in the chat.
use super::Harness;

/// Live capture shape (dialog/tests D1): question + numbered options.
fn picker_screen() -> Vec<String> {
    [
        "← ☐ Partial ☐ Story ✔ Submit →",
        "│ What would you like to do with changes?",
        "❯ 1. Discard",
        "  2. Keep",
        "  3. Test",
        "  4. Commit",
        "──────",
        "  5. Chat about this",
        "Enter to select · Tab/Arrow keys to navigate",
    ]
    .iter()
    .map(|l| l.to_string())
    .collect()
}

/// Text-input dialog (no options): the only way to answer is typing.
fn question_screen() -> Vec<String> {
    [
        "  ┃  △ Permission required",
        "  ┃    ← Access external directory ~/.config/opencode",
        "  ┃",
        "  ┃  Enter your answer:",
        "  ┃   ⬝ esc interrupt   24%  ctrl+p commands",
    ]
    .iter()
    .map(|l| l.to_string())
    .collect()
}

/// Every `callback_data` in a card's keyboard (the tap protocol the
/// buttons actually speak).
fn button_data(card_body: &serde_json::Value) -> Vec<String> {
    // Telegram wraps inline keyboards: {"inline_keyboard": [[btn]]}.
    let kb = &card_body["reply_markup"];
    let kb = if kb.is_object() {
        &kb["inline_keyboard"]
    } else {
        kb
    };
    kb.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.as_array())
                .flatten()
                .filter_map(|b| b["callback_data"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Drive a turn to the point where the agent asks.
async fn blocked(h: &Harness) {
    h.map_topic();
    h.herdr.set_status("working");
    h.say("refactor the parser").await;
    h.wait_for("submit", || h.herdr.submit_count() >= 1).await;
    h.herdr.set_screen_owned(picker_screen());
    h.herdr.set_status("blocked");
}

#[tokio::test]
async fn e2e_blocked_posts_question_card_with_option_buttons() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    // Buttons: one per option plus the dismiss row, and NOT a "type
    // answer" button (an option dialog has nowhere for free text).
    let card = h
        .calls("sendMessage")
        .into_iter()
        .find(|c| c.text().contains("What would you like to do"))
        .expect("card");
    let data = button_data(&card.body);
    assert!(
        data.iter().any(|d| d.starts_with("B:opt0:")),
        "option buttons present: {data:?}"
    );
    assert!(
        data.iter().any(|d| d.starts_with("B:deny:")),
        "dismiss present: {data:?}"
    );
    assert!(
        !data.iter().any(|d| d.starts_with("B:type:")),
        "no phantom type button on an option dialog: {data:?}"
    );
}

#[tokio::test]
async fn e2e_tapped_option_answers_the_agent_with_keys() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    // Tap the second option (Keep) ON the tracked card.
    let card = h.card_id("What would you like to do");
    h.tap_on(card, &format!("B:opt1:{}", h.pane)).await;
    h.wait_for("keys sent to the pane", || !h.herdr.keys_sent().is_empty())
        .await;
    let keys = h.herdr.keys_sent().join("|");
    assert!(
        keys.contains("2") || keys.contains("Down") || keys.contains("Arrow"),
        "navigated to the option: {keys:?}"
    );
}

#[tokio::test]
async fn e2e_deny_button_dismisses_without_waiting() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    let card = h.card_id("What would you like to do");
    h.tap_on(card, &format!("B:deny:{}", h.pane)).await;
    h.wait_for("deny reached the pane", || !h.herdr.keys_sent().is_empty())
        .await;
}

#[tokio::test]
async fn e2e_text_input_dialog_offers_type_answer() {
    let h = Harness::start().await;
    h.map_topic();
    h.herdr.set_status("working");
    h.say("do the thing").await;
    h.wait_for("submit", || h.herdr.submit_count() >= 1).await;
    h.herdr.set_screen_owned(question_screen());
    h.herdr.set_status("blocked");
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("Permission required"))
    })
    .await;
    let card = h
        .calls("sendMessage")
        .into_iter()
        .find(|c| c.text().contains("Permission required"))
        .expect("card");
    let has_type = button_data(&card.body)
        .iter()
        .any(|d| d.starts_with("B:type:"));
    assert!(has_type, "text dialog must offer a type-answer button");
}

#[tokio::test]
async fn e2e_card_command_reposts_the_question() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    let before = h
        .calls("sendMessage")
        .iter()
        .filter(|c| c.text().contains("What would you like to do"))
        .count();
    h.say("/card").await;
    h.wait_for("reposted card", || {
        h.calls("sendMessage")
            .iter()
            .filter(|c| c.text().contains("What would you like to do"))
            .count()
            > before
    })
    .await;
}

#[tokio::test]
async fn e2e_esc_dismisses_blocked_card() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    h.say("/esc").await;
    h.wait_for("card resolved", || {
        h.sent_count("deleteMessage") >= 1 || !h.herdr.keys_sent().is_empty()
    })
    .await;
}

/// A blocked settle must NOT also post a plain final card (the question
/// card is the answer surface; two cards would double-notify).
#[tokio::test]
async fn e2e_blocked_settle_posts_one_card_not_two() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    h.tick(4).await;
    let texts = h.sent_texts();
    let question_cards = texts
        .iter()
        .filter(|t| t.contains("What would you like to do"))
        .count();
    assert_eq!(question_cards, 1, "one question card only: {texts:?}");
}

/// The keyboard must target the card's own chat+thread (a card posted
/// into General would answer in the wrong place).
#[tokio::test]
async fn e2e_card_lands_in_the_pane_topic() {
    let h = Harness::start().await;
    blocked(&h).await;
    h.wait_for("question card", || {
        h.calls("sendMessage")
            .iter()
            .any(|c| c.text().contains("What would you like to do"))
    })
    .await;
    let card = h
        .calls("sendMessage")
        .into_iter()
        .find(|c| c.text().contains("What would you like to do"))
        .expect("card");
    assert_eq!(card.body["chat_id"].as_i64(), Some(h.chat));
    assert_eq!(
        card.body["message_thread_id"].as_i64(),
        Some(h.thread),
        "card must live in the pane's topic"
    );
}
