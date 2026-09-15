/// Live picker scanning for the opencode `/models` picker.
/// Pure — tested below. `picker_hit` is the switch flow's match check;
/// `picker_rows`/`free_rows` are the future `/model all` live-scan kit.
use super::model_parse::{FREE_ZEN, norm};
use crate::jobs::filter::deframe;

/// Picker row: display name + provider + free badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRow {
    pub name: String,
    pub provider: String,
    pub free: bool,
}

/// Picker rows between the `Select model` header and the
/// `Connect provider` footer. Strict column shaping — misses rows when
/// background chat text bleeds into picker rows (see `picker_hit`), so
/// the switch flow does NOT rely on this.
#[allow(dead_code)]
pub fn picker_rows(screen: &[String]) -> Vec<ModelRow> {
    let mut rows = Vec::new();
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
        if !inside || t.is_empty() {
            continue;
        }
        let cols: Vec<String> = t
            .split('\t')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        let cols = if cols.len() == 1 {
            split_columns(&cols[0])
        } else {
            cols
        };
        if cols.len() < 2 || cols.len() > 3 {
            continue;
        }
        let name = cols[0].clone();
        if name.chars().count() < 3 || !name.chars().any(|c| c.is_alphabetic()) {
            continue;
        }
        let provider = cols[1].clone();
        if provider.split_whitespace().count() > 3 || provider.chars().count() > 24 {
            continue;
        }
        let free = cols
            .get(2)
            .map(|c| c.eq_ignore_ascii_case("free"))
            .unwrap_or(false);
        rows.push(ModelRow {
            name,
            provider,
            free,
        });
    }
    rows
}

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
/// shifting positional columns and hiding the row from `picker_rows`.
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

/// Free-Zen rows among live picker rows: name matches a known free id AND
/// the provider is Zen (excludes same-name `(free)` OpenRouter rows).
#[allow(dead_code)]
pub fn free_rows(rows: &[ModelRow]) -> Vec<ModelRow> {
    rows.iter()
        .filter(|r| {
            let n = norm(&r.name);
            norm(&r.provider).contains("zen")
                && FREE_ZEN.iter().any(|(_, id, _)| n.contains(&norm(id)))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_picker_rows_bounded_and_shaped() {
        let screen = v(&[
            "     Thought · 234ms",
            "     some chatter",
            "}      Select model                                     esc",
            "     So     ling                                                    plan",
            "     Ling 3.0 Flash Fin Free                 OpenCode Zen",
            "     Nemotron 3.5 Lightning Free             OpenCode Zen",
            "     ▣      Ling 3.0 Flash VL                         OpenRouter",
            "  ┃         Connect provider ctrl+a  Favorite ctrl+f",
            "   /home/user/projects/herdr-telegram        15.5K (1%)  ctrl+p commands",
        ]);
        let rows = picker_rows(&screen);
        // The ▣-led row is rejected (no valid name column).
        assert!(
            rows.iter()
                .any(|r| r.name == "Ling 3.0 Flash Fin Free" && r.provider == "OpenCode Zen")
        );
        assert!(!rows.iter().any(|r| r.name.contains("chatter")));
        assert!(!rows.iter().any(|r| r.name.contains("ctrl+p")));
    }

    #[test]
    fn test_free_rows_excludes_openrouter() {
        let rows = vec![
            ModelRow {
                name: "Ling 3.0 Flash Fin Free".into(),
                provider: "OpenCode Zen".into(),
                free: true,
            },
            ModelRow {
                name: "Ling 3.0 Flash Fin (free)".into(),
                provider: "OpenRouter".into(),
                free: true,
            },
            ModelRow {
                name: "GPT 5.5".into(),
                provider: "OpenCode Zen".into(),
                free: false,
            },
        ];
        let free = free_rows(&rows);
        assert_eq!(free.len(), 1);
        assert_eq!(free[0].provider, "OpenCode Zen");
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
