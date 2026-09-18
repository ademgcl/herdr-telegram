/// Live picker scanning for the opencode `/models` picker.
/// Pure — tested below. `picker_hit` is the switch flow's match check.
use super::model_parse::norm;
use crate::jobs::filter::deframe;

/// Split on tabs or runs of 2+ spaces (single spaces stay in phrases).
/// Shared with `interactive::parse_options` — one implementation.
pub(crate) fn split_columns(t: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut gap = 0usize;
    for c in t.chars().chain(std::iter::once(' ')) {
        if c == '\t' {
            if gap == 1 {
                cur.pop();
            }
            if gap < 2 {
                let p = cur.trim().to_string();
                if !p.is_empty() {
                    parts.push(p);
                }
                cur.clear();
            }
            gap = 2;
        } else if c.is_whitespace() {
            gap += 1;
            if gap == 1 {
                cur.push(' ');
            } else if gap == 2 {
                cur.pop();
                let p = cur.trim().to_string();
                if !p.is_empty() {
                    parts.push(p);
                }
                cur.clear();
            }
        } else {
            gap = 0;
            cur.push(c);
        }
    }
    let p = cur.trim().to_string();
    if !p.is_empty() {
        parts.push(p);
    }
    parts
}

/// Did the filtered picker list the target? Substring match on deframed
/// picker-region lines — deliberately NOT the strict column parse above.
/// The picker renders OVER the conversation, so background chat text
/// bleeds into the same terminal rows ("Hi.    Muse Spark 1.3 Free   …"),
/// shifting positional columns, so matching stays substring-based.
/// The search-echo line (which always contains our own filter text) is
/// stripped first so it can't self-match when nothing was found.
pub fn picker_hit(screen: &[String], filter: &str, marker: &str) -> bool {
    let m = norm(marker);
    if m.is_empty() {
        return false;
    }
    let f = norm(filter);
    let mut inside = false;
    for line in screen {
        let t = deframe(line).trim().to_string();
        if t.contains("Select model") {
            inside = true;
            continue;
        }
        if t.contains("Connect provider") {
            break;
        }
        if !inside {
            continue;
        }
        let n = norm(&t);
        let body = if f.is_empty() { n } else { n.replace(&f, "") };
        if body.contains(&m) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_picker_hit_ignores_background_bleed_and_echo() {
        // Real dump: chat text ("Hi.", "Tho") shares rows with the picker.
        // The strict column parse sees name="Hi." here — substring must not.
        let screen = v(&[
            "┃         Select model                                     esc",
            "     Tho    muse spark 1.3 free zen",
            "     Hi.    Muse Spark 1.3 Free                     OpenCode Zen",
            "     ▣      Connect provider ctrl+a  Favorite ctrl+f",
        ]);
        assert!(picker_hit(
            &screen,
            "muse spark 1.3 free zen",
            "muse spark 1.3 free"
        ));
        // No Nemotron row: echo remainder ("tho") must not self-match.
        assert!(!picker_hit(
            &screen,
            "nemotron 3 ultra free zen",
            "nemotron 3 ultra free"
        ));
        assert!(!picker_hit(&screen, "muse spark 1.3 free zen", ""));
    }
}
