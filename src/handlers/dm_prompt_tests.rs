//! Tests for [`super::dm_prompt`] (split: 300-line file limit).
use super::*;

#[test]
fn test_pane_shaped_dead_vs_ordinary() {
    assert!(pane_shaped("w1:p1"));
    assert!(pane_shaped("w8:p3"));
    assert!(pane_shaped("w9:p7"));
    assert!(pane_shaped("w10:p12"));
    assert!(!pane_shaped("w1:p1extra"));
    assert!(!pane_shaped("w1x:p1"));
    assert!(!pane_shaped("note:"));
    assert!(!pane_shaped("note:p1"));
    assert!(!pane_shaped("dead:p9"));
    assert!(!pane_shaped("well:done"));
    assert!(!pane_shaped("https://foo"));
    assert!(!pane_shaped("hello"));
    assert!(!pane_shaped("opencode"));
}

#[test]
fn test_strip_addr_punct_keeps_address() {
    assert_eq!(strip_addr_punct("w8:p1,"), "w8:p1");
    assert_eq!(strip_addr_punct("w8:p1..."), "w8:p1");
    assert_eq!(strip_addr_punct("w8:p1:"), "w8:p1");
    assert_eq!(strip_addr_punct("opencode,"), "opencode");
    assert_eq!(strip_addr_punct("w1:p1"), "w1:p1");
    assert_eq!(strip_addr_punct("hello"), "hello");
    // Bare punctuation is prompt text, never the sole-agent address.
    assert_eq!(strip_addr_punct("..."), "...");
    assert_eq!(strip_addr_punct("!!!"), "!!!");
    // Fail-closed gate: punctuated kinds never count as pane-shaped,
    // so `opencode, fix` stays prompt text instead of misrouting.
    assert!(!pane_shaped(strip_addr_punct("opencode,")));
    assert!(pane_shaped(strip_addr_punct("w8:p1,")));
}
