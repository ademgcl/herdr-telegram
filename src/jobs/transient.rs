//! Shared regression inventory for transient-noise tests (test-only).
//!
//! Single source for the filter/segment/arbitrate/dialog batteries: every
//! known intermediate-work TUI shape across providers lives in TRANSIENT,
//! every must-survive prose shape in PROSE. Add new agent noise HERE with
//! its fix — the invariant test fails until the shape is handled (chrome
//! or boundary), so a future glyph can never silently become a card again.
pub(crate) const TRANSIENT: &[&str] = &[
    // Heavy-asterisk tool echoes (the reported leaks, verbatim).
    "✱ Grep \"send_msg|send_msg_with_effect|send_silent\" in src (100 matches)",
    "✱ Grep \"is_effect_rejection|effect_rejection|message effects\" in src (5 matches)",
    "✱ Read src/jobs/filter.rs (241 lines)",
    "✱ Bash cargo test (2m 3s)",
    "  ✱ Edit src/main.rs",
    // Sibling tool-call echoes, every verb/count shape.
    "→Read src/telegram/router.rs",
    "← Edit src/jobs/reply.rs",
    "● Bash(cargo test)",
    "○ Read(src/lib.rs)",
    "⏺ Write(src/main.rs)",
    "⎿ Done in 1.2s",
    "☰ Apply patch",
    "❯ Run tests",
    "※ recap: completed step 1",
    "✻ Cooked for 5m 36s · done",
    // Tilde progress (the reported leak) + sibling progress verbs.
    "~ Writing command…",
    "~ Writing command...",
    "  ~ Reading files…",
    "~",
    "Writing…",
    "Working…",
    "Working... 12s",
    "Thinking…",
    "⡿ Running command...",
    "⠋ Syncing…",
    // Reasoning headers, step cards, expanders.
    "   Thought · 359ms",
    "+ Thought: 6.8s",
    "▸ Thought for 11s, 1.5k tokens",
    "  ▣  Build · Muse Spark 1.3 Free · 1m 3s",
    "Click to expand",
    "• OpenCode 1.18.31",
    "# Todos",
    "[✓] Add session_id to AgentDetail",
    // Footers, rules, frames, bot log lines.
    "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent",
    "? for shortcuts             Gemini 3.8 Flash · high",
    "────────────────────────────────────────────────",
    "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
    "… +4 lines (ctrl + t to view transcript)",
    "[watcher] start wG:p1",
    "   /home/user/projects/herdr-telegram        10.8K (1%)  ctrl+p commands",
    // Agy shapes.
    "Antigravity CLI 1.2.2",
    "  ADC: firebase-adminsdk-fbsvc@test-proj-123",
    "└ Tip: Run with --nocapture",
];

/// Reply prose that must survive filtering byte-for-byte. Each entry guards
/// a narrowness rule: mid-line glyphs, ASCII bullets, checkmarks, ~/paths,
/// leading "Writing " (no ellipsis), trailing ellipses and code fences are
/// content, never chrome.
pub(crate) const PROSE: &[&str] = &[
    "The guard was missing — fixed and tested.",
    "✅ Done — all 78 tests pass",
    "* bullet item",
    "~/projects/herdr-telegram",
    "~/.config/opencode/allowlist",
    "I am writing the migration guide now",
    "Writing tests first, then the fix",
    "✓ Done!",
    "let next = a → b;",
    "use ● as the marker",
    "The fix is ready…",
    "100 matches found in src",
    "> quoted text",
    "| a | b |",
    "```rust",
    "Hi! I'm Muse Spark. How can I help today?",
];
