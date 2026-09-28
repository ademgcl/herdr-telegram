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

/// Adaptive read for live streaming.
///
/// VISIBLE FIRST. The `recent_unwrapped` fallback is not a cheap retry:
/// on a busy alternate-screen agent herdr runs its alt-screen read path
/// — which probes the pane's bottom — and only then rejects the capture
/// with `agent_not_idle`. Asking first meant paying that probe on every
/// poll tick, for every pane, and never getting the data back: the log
/// is a wall of `outcome=error` from exactly this call, and the probe is
/// what drags a scrolled-back pane to the bottom.
///
/// `visible` is the correct primary source anyway for live streaming —
/// it is the pane's current state, and it is the only source herdr
/// serves while an agent is blocked. The wide recent read stays as the
/// fallback for when the viewport genuinely comes back empty, which is
/// the only case it was ever good for.
pub async fn read_screen_adaptive(socket: &str, pane: &str) -> Vec<String> {
    let tail = read_agent_visible(socket, pane, LIVE_TAIL_LINES).await;
    if let Ok(text) = tail
        && !text.trim().is_empty()
    {
        return wrap(text);
    }
    match read_agent_output(socket, pane, 200).await {
        Ok(text) if !text.is_empty() => wrap(text),
        _ => Vec::new(),
    }
}

/// Shortest space run that votes for a column split. Small on purpose:
/// the cut lands on the agreed COLUMN, so a long line leaving a narrow
/// gap is still caught.
const MIN_GUTTER: usize = 3;

/// A screen narrower than this has no sidebar column to find.
const MIN_SIDEBAR_WIDTH: usize = 80;

/// The split must sit at least this far into the screen, in tenths.
const SPLIT_FRACTION: usize = 6;

/// Rows that must agree before a column is believed to be the sidebar's
/// left edge. Real capture: 37 of 60 rows vote for column 171.
const MIN_VOTES: usize = 3;

/// Narrowest screen that can carry a sidebar, and how far right the split
/// must sit (as a fraction of the widest row).
///
/// A row with text, a gap, then more text is ambiguous on its own —
/// prose with aligned columns looks identical. What settles it is WHERE:
/// a sidebar occupies the right of a WIDE terminal (the real pane splits
/// at 171 of ~210 columns), while aligned prose gaps sit near the start.
/// Without this, four aligned lines of `"one<gap>two"` truncate themselves.
/// A two-column TUI (Kilo and friends) glues a LIVE sidebar — model,
/// `Steps`/`Cost`, context/cache meters, modified files — onto the SAME
/// lines as the conversation. That breaks everything downstream:
/// consecutive reads never match line-for-line (the counters move), so
/// the line-exact delta anchor missed and its cut landed ABOVE the new
/// output, re-serving prior-turn scrollback; and the sidebar rode
/// straight into replies and the transient.
///
/// A fixed WIDTH threshold cannot work: the sidebar sits at a fixed
/// COLUMN, so the gap before it SHRINKS as conversation text grows, and
/// long lines left three or four spaces that slipped past the cut. So
/// vote for the split instead — the gap END column with text on BOTH
/// sides, most-voted wins. The shared indent never qualifies (nothing to
/// its left) and a trailing gap never does (nothing to its right).
fn sidebar_column(lines: &[&str]) -> Option<usize> {
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    if widest < MIN_SIDEBAR_WIDTH {
        return None;
    }
    let min_col = widest * SPLIT_FRACTION / 10;
    let mut votes: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for l in lines {
        let b: Vec<char> = l.chars().collect();
        let mut run = 0usize;
        for (i, c) in b.iter().enumerate() {
            if *c == ' ' {
                run += 1;
            } else {
                let (rs, re) = (i - run, i);
                if run >= MIN_GUTTER && b[..rs].iter().any(|c| !c.is_whitespace()) {
                    *votes.entry(re).or_default() += 1;
                }
                run = 0;
            }
        }
    }
    votes
        .into_iter()
        .filter(|(c, n)| *n >= MIN_VOTES && *c >= min_col)
        .max_by_key(|(c, n)| (*n, std::cmp::Reverse(*c)))
        .map(|(c, _)| c)
}

/// A sidebar's cells are WILDLY uneven — `▼ Context` next to
/// `▶ Space Bunny Alpha …   204     $0.00` next to a file diff. Aligned
/// body text (a code table, a markdown table) is the opposite: the cells
/// to the right of a gap are near-identical in length, row after row.
///
/// That difference is what separates a real sidebar from a body that
/// merely has aligned columns, and it matters: the geometry alone
/// truncates single-column screens for the other 17 supported agents,
/// silently eating part of a real answer.
fn sidebar_cells_vary(cells: &[String]) -> bool {
    if cells.len() < 3 {
        return false;
    }
    let lens: Vec<usize> = cells.iter().map(|c| c.trim().chars().count()).collect();
    let max = lens.iter().max().copied().unwrap_or(0);
    let min = lens.iter().min().copied().unwrap_or(0);
    // A sidebar spans a wide range of cell widths; a table's columns do
    // not. Requiring a real spread also rejects a sidebar of one-token
    // labels, which is not what these TUIs paint.
    max >= 12 && max.saturating_sub(min) >= 8
}

/// Screen lines with the sidebar column cut off. A screen with no
/// agreed column passes through untouched, so ordinary prose with wide
/// gaps is never truncated.
fn strip_columns(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(col) = sidebar_column(&lines) else {
        return lines.iter().map(|l| l.trim_end().to_string()).collect();
    };
    // Reject when the right-hand cells are uniform: that is an aligned
    // body, not a sidebar, and cutting it would truncate a real reply.
    let cells: Vec<String> = lines
        .iter()
        .filter_map(|l| {
            l.chars()
                .nth(col)
                .map(|_| l.chars().skip(col).collect::<String>())
        })
        .filter(|c: &String| !c.trim().is_empty())
        .collect();
    if !sidebar_cells_vary(&cells) {
        return lines.iter().map(|l| l.trim_end().to_string()).collect();
    }
    lines
        .iter()
        .map(|l| {
            let cut: String = l.chars().take(col).collect();
            cut.trim_end().to_string()
        })
        .collect()
}

fn wrap(text: String) -> Vec<String> {
    strip_columns(&text)
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
    use super::strip_columns;

    /// Rows captured verbatim from the live Kilo pane, split at the real
    /// sidebar edge: every row's conversation is padded out to the SAME
    /// column (171 on the live pane) and the furniture hangs off it. These
    /// are the narrow-gap rows that beat a 20-space threshold and shipped
    /// `Context`, `Cache rate` and `src/jobs/progress.rs +20 -8` into the
    /// card — the gap shrinks as the conversation gets longer, the column
    /// never moves.
    const ROWS: &[(&str, &str)] = &[
        (
            "     Still outstanding: the duplicate instant post.",
            "\u{25bc} Reasoning",
        ),
        (
            "     I would rather not touch the threshold on a guess.",
            "Cache rate",
        ),
        (
            "     Deployed: b440a47, PID 22360, tree clean.",
            "src/jobs/progress.rs   +20 -8",
        ),
    ];

    /// Rebuild a screen the way the TUI paints it: shared column edge.
    fn screen(edge: usize) -> String {
        ROWS.iter()
            .map(|(l, r)| {
                let n = l.chars().count();
                format!("{l}{}{r}", " ".repeat(edge.saturating_sub(n)))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_narrow_gap_rows_lose_the_sidebar() {
        let joined = strip_columns(&screen(80)).join("\n");
        for keep in ["duplicate instant", "not touch the threshold", "b440a47"] {
            assert!(
                joined.contains(keep),
                "conversation lost {keep:?}: {joined:?}"
            );
        }
        for junk in ["Reasoning", "Cache rate", "progress.rs", "+20 -8"] {
            assert!(
                !joined.contains(junk),
                "sidebar survived: {junk:?} in {joined:?}"
            );
        }
    }

    #[test]
    fn test_shared_indent_is_never_the_column() {
        let cut = strip_columns(&screen(80));
        assert!(
            cut.iter().all(|l| l.starts_with("     ")),
            "cut inside the indent: {cut:?}"
        );
    }

    /// One column of agreement is noise, not a sidebar.
    #[test]
    fn test_a_single_agreeing_row_is_not_a_sidebar() {
        let text = "left column here            right one\nplain line\nanother line";
        assert_eq!(strip_columns(text).join("\n"), text);
    }

    /// Ordinary prose with wide gaps must survive untouched: without an
    /// agreed column there is nothing to cut on.
    #[test]
    fn test_single_column_prose_is_untouched() {
        let text = "one        two\nthree      four\nfive       six\nseven      eight";
        assert_eq!(strip_columns(text).join("\n"), text);
    }
}

#[cfg(test)]
#[path = "screens_single_column_tests.rs"]
mod single_column_tests;
