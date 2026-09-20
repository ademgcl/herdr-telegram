//! Tests for [`super::client`] (split: 300-line file limit).
use super::*;

fn test_client() -> TelegramClient {
    TelegramClient::new("fake-token-123".to_string()).expect("client build")
}

#[test]
fn redact_replaces_token() {
    let c = test_client();
    let out = c.redact("https://api.telegram.org/botfake-token-123/getUpdates failed");
    assert!(!out.contains("fake-token-123"));
    assert!(out.contains("<redacted>"));
}

#[test]
fn test_redact_leaves_clean_input_unchanged() {
    let c = test_client();
    assert_eq!(c.redact("connection reset"), "connection reset");
}

#[test]
fn test_is_unauthorized_matches_token_death_only() {
    // Real revoked-token shape: Telegram's Unauthorized description.
    assert!(TelegramClient::is_unauthorized("Unauthorized"));
    assert!(TelegramClient::is_unauthorized(
        "Unauthorized: bot was kicked"
    ));
    // Real deleted-token shape: HTTP 404 "Not Found" (the 401 classifier
    // alone never fires for it — a dead bot would retry forever).
    assert!(TelegramClient::is_unauthorized("Not Found"));
    // A bare "401" substring is NOT enough: retry intervals, message
    // text, and chat ids all contain it — must not FATAL-exit.
    assert!(!TelegramClient::is_unauthorized("retry after 401s"));
    assert!(!TelegramClient::is_unauthorized("message 401 ok"));
    assert!(!TelegramClient::is_unauthorized("connection reset"));
    // Contextual "not found" inside an otherwise-live error (the title's
    // text) is not a dead token — only Telegram's bare Not Found body is.
    assert!(!TelegramClient::is_unauthorized(
        "message to edit not found"
    ));
}

#[test]
fn test_is_transient_msg_matches_server_strings() {
    // Telegram 5xx arrives as plain strings via `call` (never a reqwest
    // downcast): the loud send path must retry these like call_retrying.
    for s in [
        "Internal Server Error",
        "Bad Gateway",
        "Service Unavailable",
        "Gateway Timeout",
        "connection reset",
        "request timed out",
    ] {
        assert!(TelegramClient::is_transient_msg(s), "missed: {s}");
    }
    assert!(!TelegramClient::is_transient_msg("Unauthorized"));
    assert!(!TelegramClient::is_transient_msg(
        "message to edit not found"
    ));
    assert!(!TelegramClient::is_transient_msg("connection refused"));
}

#[test]
fn test_retry_after_parsing() {
    use std::time::Duration;
    // +1s bias on top of the asked wait.
    assert_eq!(
        TelegramClient::retry_after("Too Many Requests: retry after 30"),
        Some(Duration::from_secs(31))
    );
    assert_eq!(
        TelegramClient::retry_after("retry after 0"),
        Some(Duration::from_secs(1))
    );
    // Colon/prefix shapes: the numeric run follows non-digit separators.
    assert_eq!(
        TelegramClient::retry_after("Too Many Requests: retry after: 30"),
        Some(Duration::from_secs(31))
    );
    assert_eq!(
        TelegramClient::retry_after("retry after in 30s"),
        Some(Duration::from_secs(31))
    );
    assert_eq!(TelegramClient::retry_after("connection reset"), None);
    assert_eq!(TelegramClient::retry_after("retry after many"), None);
    // Digits without the marker are never a flood-wait (rsplit().next()
    // is never None, so the old code slept ~60s on any numeric error).
    assert_eq!(
        TelegramClient::retry_after("Bad Request: message 123 not found"),
        None
    );
    assert_eq!(
        TelegramClient::retry_after("error sending request bot123456:ABC/sendMessage"),
        None
    );
}

#[test]
fn test_retry_after_caps_huge_waits_and_overflow() {
    use std::time::Duration;
    // Uncapped, `retry after 1000000` would stall the caller for ~11
    // days — the cap bounds every retry loop (≤3 waits × cap).
    assert_eq!(
        TelegramClient::retry_after("Too Many Requests: retry after 1000000"),
        Some(Duration::from_secs(TelegramClient::MAX_FLOOD_WAIT_SECS))
    );
    // u64::MAX parses but must not overflow the +1 bias.
    assert_eq!(
        TelegramClient::retry_after(&format!("retry after {}", u64::MAX)),
        Some(Duration::from_secs(TelegramClient::MAX_FLOOD_WAIT_SECS))
    );
    // Small waits keep the +1s bias under the cap.
    assert_eq!(
        TelegramClient::retry_after("retry after 30"),
        Some(Duration::from_secs(31))
    );
}

#[test]
fn test_redact_empty_token_leaves_input_unchanged() {
    let c = TelegramClient::new(String::new()).expect("client build");
    assert_eq!(c.redact("connection reset"), "connection reset");
    assert_eq!(c.redact(""), "");
}

#[test]
fn test_menu_names_tags_and_no_alias() {
    // Scope contract (not prose): every menu name, its tag shape, and
    // the deleted alias staying absent. Tags = full-function scope;
    // untagged = responds on every surface (full or guidance).
    // (topic/DM)-tagged: full function there + General redirect.
    let tagged = [
        "model", "quit", "kill", "split", "read", "output", "status", "history", "card", "esc",
        "keys",
    ];
    // Global: full, redirect, or refusal on every surface.
    let global = [
        "start", "agents", "spawn", "space", "shell", "pane", "cancel", "help",
    ];
    let names: Vec<&str> = TelegramClient::MENU_COMMANDS
        .iter()
        .map(|(c, _)| *c)
        .collect();
    assert_eq!(names.len(), tagged.len() + global.len() + 1, "menu grew?");
    for cmd in tagged {
        let desc = TelegramClient::MENU_COMMANDS
            .iter()
            .find(|(c, _)| *c == cmd)
            .unwrap()
            .1;
        assert!(desc.contains("(topic/DM)"), "{cmd} lost its scope tag");
    }
    for cmd in global {
        let desc = TelegramClient::MENU_COMMANDS
            .iter()
            .find(|(c, _)| *c == cmd)
            .unwrap()
            .1;
        assert!(!desc.contains("(topic/DM)"), "{cmd} gained a scope tag");
    }
    let reset = TelegramClient::MENU_COMMANDS
        .iter()
        .find(|(c, _)| *c == "reset")
        .unwrap()
        .1;
    assert!(reset.contains("paced"), "reset lost its paced marker");
    assert!(
        !TelegramClient::MENU_COMMANDS
            .iter()
            .any(|(c, _)| *c == "reset_topics"),
        "deleted alias resurrected in menu"
    );
}
