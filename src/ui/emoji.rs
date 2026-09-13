/// NOTE: ✅/⛔/⚪ default to *text* presentation (U+2705 etc. bare) and
/// Telegram's topic-name comparison drops them — state badges using them
/// silently never apply. The explicit \u{fe0f} (VS16) forces real emoji.
/// 🔄/🟢 are emoji-presentation by default and need nothing.
pub fn emoji(status: &str) -> &'static str {
    match status {
        "working" => "🔄",
        "idle" => "🟢",
        "blocked" => "⛔\u{fe0f}",
        "done" => "✅\u{fe0f}",
        "closed" | "dead" | "exited" => "⚪\u{fe0f}",
        _ => "❔",
    }
}

pub fn worst_status<'a, I>(statuses: I) -> &'static str
where
    I: IntoIterator<Item = &'a str>,
{
    let mut has_idle = false;
    let mut has_done = false;
    let mut has_working = false;

    for s in statuses {
        if s == "blocked" {
            return "⛔\u{fe0f}";
        }
        if s == "working" {
            has_working = true;
        } else if s == "done" {
            has_done = true;
        } else if s == "idle" {
            has_idle = true;
        }
    }

    if has_working {
        "🔄"
    } else if has_done {
        "✅\u{fe0f}"
    } else if has_idle {
        "🟢"
    } else {
        "▫️"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emoji_mapping() {
        assert_eq!(emoji("working"), "🔄");
        assert_eq!(emoji("idle"), "🟢");
        assert_eq!(emoji("blocked"), "⛔\u{fe0f}");
        assert_eq!(emoji("done"), "✅\u{fe0f}");
        assert_eq!(emoji("exited"), "⚪\u{fe0f}");
        assert_eq!(emoji("other"), "❔");
    }

    #[test]
    fn test_worst_status() {
        assert_eq!(worst_status(vec!["idle", "working", "blocked"]), "⛔\u{fe0f}");
        assert_eq!(worst_status(vec!["idle", "working"]), "🔄");
        assert_eq!(worst_status(vec!["idle", "done"]), "✅\u{fe0f}");
        assert_eq!(worst_status(vec!["idle"]), "🟢");
        assert_eq!(worst_status(vec![]), "▫️");
    }
}
