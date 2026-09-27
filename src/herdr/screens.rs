//! Terminal screen snapshots over herdr socket RPC.
use super::agents::{read_agent_output, read_agent_visible};

/// Visible-viewport snapshot — works on blocked panes where the
/// `recent_unwrapped` source errors out.
pub async fn read_screen_visible(socket: &str, pane: &str, lines: u32) -> Vec<String> {
    read_agent_visible(socket, pane, lines)
        .await
        .map(wrap)
        .unwrap_or_default()
}

/// Adaptive read for live streaming: herdr rejects large captures on busy
/// alternate-screen TUIs (any supported agent) but allows small visible tails;
/// line-printing agents allow large reads. Use the biggest window available.
pub async fn read_screen_adaptive(socket: &str, pane: &str) -> Vec<String> {
    let big = read_agent_output(socket, pane, 200).await;
    match big {
        Ok(text) if !text.is_empty() => wrap(text),
        _ => {
            let tail = read_agent_visible(socket, pane, LIVE_TAIL_LINES).await;
            tail.map(wrap).unwrap_or_default()
        }
    }
}

/// Consecutive spaces that mean "column gutter", not indentation.
///
/// A two-column TUI (Kilo and friends) glues a LIVE sidebar — model
/// name, `Steps`/`Cost`, token counters, modified files, `esc
/// interrupt` — onto the SAME lines as the conversation, separated by a
/// run of padding. That breaks everything downstream: consecutive reads
/// never match line-for-line (the counters move), so the line-exact
/// delta anchor missed and its fallback re-served PRIOR-TURN scrollback
/// as this turn's output; and the sidebar text rode along into replies
/// and the transient. Cutting each line at its gutter keeps the
/// conversation column and drops the furniture.
///
/// 20, not 8: the widest real indentation (nested code) stays under it,
/// while a TUI gutter is far wider.
const GUTTER_SPACES: usize = 20;

/// One screen line, truncated at the first column gutter.
fn strip_gutter(line: &str) -> String {
    let mut run = 0usize;
    for (i, c) in line.char_indices() {
        if c == ' ' {
            run += 1;
            if run >= GUTTER_SPACES {
                return line[..i].trim_end().to_string();
            }
        } else {
            run = 0;
        }
    }
    line.trim_end().to_string()
}

fn wrap(text: String) -> Vec<String> {
    text.lines().map(strip_gutter).collect()
}

/// Visible-tail width for blocked panes. Kept equal to
/// `handlers::dialog::DIALOG_READ_LINES` (single source would be a
/// layer inversion): blocked-card stamp and sig-compare windows must
/// match or tall dialogs double-buzz.
pub const LIVE_TAIL_LINES: u32 = 60;

/// Wide-window read for limit/stall scans: quota banners scroll far above
/// the live tail during hours-long auto-retry loops that print every
/// second (500 lines ≈ minutes of spam). Try the biggest window first,
/// degrade gracefully on busy alternate-screen TUIs (the 60-line visible
/// fallback matches the dialog window there). Busy TUIs pay
/// up to 3 RPCs per scan (500 + 200 fail, 60 lands) — accepted: limit
/// scans run at most every 5s per prompt pane, 60s per idle pane.
pub async fn read_screen_for_limits(socket: &str, pane: &str) -> Vec<String> {
    // Total budget: a sick herdr must not stall the 5s watcher / 60s
    // watchdog per pane (30s per-RPC timeouts × 3 fallbacks ≈ 90s).
    // Timeout reads as outage (empty): state preserved, next tick retries.
    tokio::time::timeout(std::time::Duration::from_secs(45), read_wide(socket, pane))
        .await
        .unwrap_or_default()
}

async fn read_wide(socket: &str, pane: &str) -> Vec<String> {
    if let Ok(text) = read_agent_output(socket, pane, 500).await
        && !text.trim().is_empty()
    {
        return wrap(text);
    }
    if let Ok(text) = read_agent_output(socket, pane, 200).await
        && !text.trim().is_empty()
    {
        return wrap(text);
    }
    read_agent_visible(socket, pane, LIVE_TAIL_LINES)
        .await
        .map(wrap)
        .unwrap_or_default()
}

#[cfg(test)]
mod gutter_tests {
    use super::{GUTTER_SPACES, strip_gutter};

    /// Verbatim rows from the live Kilo pane (via `herdr pane read`): the
    /// conversation and a churning sidebar share each line.
    const LIVE: &[&str] = &[
        "  ┃         herdr --session <name> [options]                                                                                                                               ▼ Models (1)",
        "  ┃         herdr session attach <name>                                                                                                                                             Kilo Gateway",
        "  ┃         herdr update [--handoff]                                                                                                                                       Model               Steps      Cost",
        "  ┃         herdr pane get                                                                                                                                                ▶ Space Bunny Alpha …   204     $0.00",
        "  ┃  …                                                                                                                                                                     Code Indexing",
        "  ┃  Click to expand                                                                                                                                                        LSP",
        "     herdr pane read is exactly what I need — the real screen.                                                                                                             Memory",
        "  ┃         herdr --remote <ssh-target> [--session]                                                                                                                         Kilo Gateway · max",
    ];

    #[test]
    fn test_sidebar_is_cut_and_conversation_kept() {
        let kept: Vec<String> = LIVE.iter().map(|l| strip_gutter(l)).collect();
        // The conversation survives verbatim.
        assert!(kept[0].contains("herdr --session <name> [options]"));
        assert!(kept[1].contains("herdr session attach <name>"));
        assert!(kept[3].contains("herdr pane get"));
        // Every piece of churning furniture is gone.
        for junk in [
            "Models (1)",
            "Kilo Gateway",
            "Steps",
            "Cost",
            "Space Bunny Alpha",
            "Code Indexing",
            "LSP",
            "Memory",
        ] {
            assert!(
                !kept.iter().any(|l| l.contains(junk)),
                "sidebar survived: {junk:?} in {kept:?}"
            );
        }
    }

    #[test]
    fn test_gutter_cut_makes_consecutive_reads_comparable() {
        // The actual failure: same sentence, sidebar counters moved. Line
        // -exact anchoring could never match, so the delta fallback cut
        // above the new output and re-served the previous turn.
        let before = strip_gutter(LIVE[3]);
        let after = strip_gutter(
            "  ┃         herdr pane get                                                                                                                                                ▶ Space Bunny Alpha …   262.4K (26%)  $0.07",
        );
        assert_eq!(
            before, after,
            "counter churn must not change the conversation line"
        );
    }

    #[test]
    fn test_normal_indentation_is_never_cut() {
        // Real content: nested code and indented prose must survive.
        for line in [
            "        let x = 1;",
            "    - a bullet",
            "  ┃         nested code stays",
            "a  b  c",
        ] {
            assert_eq!(strip_gutter(line), line.trim_end(), "cut: {line:?}");
        }
        // Must out-worst-case indentation (checked at compile time).
        const { assert!(GUTTER_SPACES > 8) };
    }

    #[test]
    fn test_opencode_status_bar_keeps_its_marker() {
        // The existing inventory shape: truncated, still recognisable as
        // chrome by its leading marker (never mistaken for a reply).
        let row = strip_gutter(
            " ⬝⬝⬝⬝⬝⬝⬝⬝ esc interrupt                                                                                                                  145.6K (14%)  ctrl+p commands    ~/projects/herdr-telegram:main",
        );
        assert!(row.contains("esc interrupt"));
        assert!(!row.contains("145.6K"));
    }
}
