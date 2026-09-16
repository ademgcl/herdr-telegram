//! Buzzing card body for a limit/stall episode. Split from `notices`
//! (300-line file limit): detection lives in `notices::detect`, rendering
//! lives here and is re-exported (`crate::jobs::notices::limit_card_text`
//! keeps working for `runner`/`reconcile`).
use super::types::{ERROR_KIND, LimitHit};

/// Buzzing card body for a limit episode. Names the pane so DM owners
/// with several agents know which one stalled.
pub fn limit_card_text(pane: &str, hit: &LimitHit) -> String {
    let (head, hint) = match hit.kind {
        "auth" => (
            "⚠️ agent auth failed",
            "Check login / API key on the PC, then prompt again.",
        ),
        ERROR_KIND => (
            "⚠️ provider request failed",
            "This run failed — it won't auto-retry. Prompt again; if it repeats, /new or /model.",
        ),
        "provider" => (
            "⚠️ provider overloaded",
            "The agent retries on its own — wait or try /model later.",
        ),
        _ => (
            "⚠️ usage limit hit — agent is auto-retrying",
            "Wait for reset, or /model to switch to a free Zen model.",
        ),
    };
    let mut text = format!("{head} [{pane}]");
    if !hit.excerpt.is_empty() {
        text.push_str(&format!("\n\n> {}", hit.excerpt));
    }
    text.push_str(&format!(
        "\n\n{hint}\n• /read — full output\n• /cancel — stop the run"
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_text_names_pane() {
        let hit = LimitHit {
            kind: "rate-limit",
            excerpt: "Free usage exceeded".into(),
            strong: true,
        };
        let text = limit_card_text("wG:p1", &hit);
        assert!(text.contains("wG:p1"));
        assert!(text.contains("Free usage exceeded"));
        assert!(text.contains("/model"));
    }

    #[test]
    fn test_provider_card_text() {
        let hit = LimitHit {
            kind: "provider",
            excerpt: "Provider response headers timed out after 300000ms [retrying attempt #1]"
                .into(),
            strong: true,
        };
        let text = limit_card_text("w8:p1", &hit);
        assert!(text.contains("w8:p1"));
        assert!(!text.contains("usage limit"));
        assert!(text.contains("/read"));
    }

    #[test]
    fn test_fatal_card_text() {
        let hit = LimitHit {
            kind: ERROR_KIND,
            excerpt: "Error from provider (Console): fail".into(),
            strong: true,
        };
        let text = limit_card_text("w8:p1", &hit);
        assert!(text.contains("won't auto-retry"));
    }
}
