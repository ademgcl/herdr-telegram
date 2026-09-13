/// NOTE: text-presentation-default codepoints (bare U+2705 etc.) are
/// dropped by Telegram's topic-name comparison — state badges using them
/// silently never apply (this bit us with ✅ and ⭐/U+2B50). Every pick
/// below is emoji-presentation by default; new glyphs must be probe-
/// verified (toggle on/off must both return ok:true, not NOT_MODIFIED).
pub fn emoji(status: &str) -> &'static str {
    match status {
        "working" => "🔄",
        "idle" => "🟢",
        "blocked" => "🛑",
        "done" => "🏆",
        "closed" | "dead" | "exited" => "💀",
        _ => "🔮",
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
            return "🛑";
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
        "🏆"
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
        assert_eq!(emoji("blocked"), "🛑");
        assert_eq!(emoji("done"), "🏆");
        assert_eq!(emoji("exited"), "💀");
        assert_eq!(emoji("other"), "🔮");
    }

    #[test]
    fn test_worst_status() {
        assert_eq!(worst_status(vec!["idle", "working", "blocked"]), "🛑");
        assert_eq!(worst_status(vec!["idle", "working"]), "🔄");
        assert_eq!(worst_status(vec!["idle", "done"]), "🏆");
        assert_eq!(worst_status(vec!["idle"]), "🟢");
        assert_eq!(worst_status(vec![]), "▫️");
    }
}
