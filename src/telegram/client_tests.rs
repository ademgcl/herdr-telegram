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
    assert!(!TelegramClient::is_unauthorized(
        "Bad Request: message thread not found"
    ));
    assert!(!TelegramClient::is_unauthorized(
        "Bad Request: Message thread not found"
    ));
    assert!(!TelegramClient::is_unauthorized(
        "Bad Request: chat not found"
    ));
    // Wrapped non-JSON 404: `call` maps decode failures to
    // `telegram http 404: Not Found (…)` — body-preserved marker,
    // no `service unavailable` shield.
    assert!(TelegramClient::is_unauthorized(
        "telegram http 404: Not Found (cannot parse response)"
    ));
    // Status Display injects "Not Found" into every `http 404 Not Found`
    // wrapper — a proxy/intermediary HTML 404 must never FATAL-exit a
    // healthy daemon (only bare/token-death shapes are unauthorized).
    assert!(!TelegramClient::is_unauthorized(
        "telegram http 404 Not Found: service unavailable (<html>404 Not Found</html> unexpected token)"
    ));
    assert!(!TelegramClient::is_unauthorized(
        "telegram http 404 Not Found: service unavailable ((unexpected)"
    ));
    // A proxy-404 wrapper without a Not Found marker is a blip, never
    // token death (must not FATAL-exit a healthy daemon into a stop).
    assert!(!TelegramClient::is_unauthorized(
        "telegram http 404: service unavailable (<html>proxy error)"
    ));
    // Status-injected 401 must never FATAL: StatusCode Display writes
    // `401 Unauthorized:` into every wrapper, and the `service
    // unavailable` arm (proxy/intermediary fault) never carries a real
    // body marker past the shield.
    assert!(!TelegramClient::is_unauthorized(
        "telegram http 401 Unauthorized: service unavailable (<html>proxy auth required)"
    ));
    // A body-preserved Unauthorized marker (raw body arm, no shield)
    // IS a dead token even inside a wrapper.
    assert!(TelegramClient::is_unauthorized(
        "telegram http 401 Unauthorized: Unauthorized (cannot parse response)"
    ));
    // But a bare 404-ish number elsewhere is not token death.
    assert!(!TelegramClient::is_unauthorized("retry after 404s"));
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
        "connection refused",
        "connection closed",
        "network is unreachable",
        "temporary failure",
    ] {
        assert!(TelegramClient::is_transient_msg(s), "missed: {s}");
    }
    assert!(!TelegramClient::is_transient_msg("Unauthorized"));
    assert!(!TelegramClient::is_transient_msg(
        "message to edit not found"
    ));
    // A 429 without a parsable retry-after still retries (backoff),
    // never fails fast and drops the buzz.
    for s in [
        "Too Many Requests: retry after in 30s",
        "Too Many Requests: flood control exceeded",
        "FLOOD_WAIT_30",
    ] {
        assert!(TelegramClient::is_transient_msg(s), "missed: {s}");
    }
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
    // Non-adjacent digits are not a flood-wait (false positive: would
    // sleep ~60s on an unrelated code).
    assert_eq!(TelegramClient::retry_after("retry after in 30s"), None);
    assert_eq!(
        TelegramClient::retry_after("retry after many (code 123)"),
        None
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
fn test_retry_after_uncapped_so_callers_can_fail_fast() {
    use std::time::Duration;
    // Uncapped: over-cap waits surface raw so `flood_wait_exceeds_cap`
    // fails the caller fast (cap-in-parse made 11-day waits look
    // sleepable and stalled a tick for the full 60s every retry).
    assert_eq!(
        TelegramClient::retry_after("Too Many Requests: retry after 1000000"),
        Some(Duration::from_secs(1_000_001))
    );
    // u64::MAX parses but must not overflow the +1 bias.
    assert_eq!(
        TelegramClient::retry_after(&format!("retry after {}", u64::MAX)),
        Some(Duration::from_secs(u64::MAX))
    );
    // Within-cap waits keep the +1s bias and are sleepable.
    assert_eq!(
        TelegramClient::retry_after("retry after 30"),
        Some(Duration::from_secs(31))
    );
    // The cap gate is the single fail-fast verdict for every caller.
    assert!(!TelegramClient::flood_wait_exceeds_cap(
        Duration::from_secs(31)
    ));
    assert!(TelegramClient::flood_wait_exceeds_cap(Duration::from_secs(
        1_000_001
    )));
}

#[test]
fn test_send_stage_transient_includes_mid_request_reset() {
    // Pure flag matrix: mid-request resets (`is_request`) must tag
    // transient — connect/timeout alone miss "connected then dropped".
    // Mirrors `send_stage_transient`'s OR (reqwest::Error is not
    // constructible in tests).
    let classified = |is_connect: bool, is_timeout: bool, is_request: bool, server: bool| {
        is_connect || is_timeout || is_request || server
    };
    assert!(classified(false, false, true, false), "mid-request reset");
    assert!(classified(true, false, false, false), "connect refused");
    assert!(classified(false, true, false, false), "deadline");
    assert!(classified(false, false, false, true), "HTTP 5xx");
    assert!(
        !classified(false, false, false, false),
        "builder/redirect never transient"
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
        "start",
        "agents",
        "spawn",
        "space",
        "shell",
        "pane",
        "cancel",
        "transient",
        "help",
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
