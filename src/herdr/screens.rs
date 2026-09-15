use super::agents::{read_agent_output, read_agent_visible};

/// Terminal snapshot as trimmed lines (delta baseline / report source).
pub async fn read_screen(socket: &str, pane: &str, lines: u32) -> Vec<String> {
    read_agent_output(socket, pane, lines)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Visible-viewport snapshot — works on blocked panes where `read_screen`
/// errors out.
pub async fn read_screen_visible(socket: &str, pane: &str, lines: u32) -> Vec<String> {
    read_agent_visible(socket, pane, lines)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
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

fn wrap(text: String) -> Vec<String> {
    text.lines().map(|l| l.trim_end().to_string()).collect()
}

pub const LIVE_TAIL_LINES: u32 = 40;
