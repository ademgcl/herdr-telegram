use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use crate::types::Res;

/// Events a prompt watcher cares about, pushed by herdr.
pub enum WatchEvent {
    /// Pane produced new output (scroll snapshot changed).
    Output,
    /// Agent semantic state transitioned.
    Status,
}

/// Persistent per-pane subscription to `pane.scroll_changed` and
/// `pane.agent_status_changed` on its own unix socket.
pub struct EvStream {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
}

impl EvStream {
    pub async fn open(socket: &str, pane: &str) -> Res<Self> {
        let conn = UnixStream::connect(socket).await?;
        let (reader, mut writer) = conn.into_split();
        let req = json!({
            "id": "watch",
            "method": "events.subscribe",
            "params": {"subscriptions": [
                {"type": "pane.scroll_changed", "pane_id": pane},
                {"type": "pane.agent_status_changed", "pane_id": pane},
            ]}
        });
        writer.write_all(format!("{req}\n").as_bytes()).await?;
        writer.flush().await?;

        let mut reader = BufReader::new(reader);
        let mut ack = String::new();
        reader.read_line(&mut ack).await?;
        if ack.contains("\"error\"") {
            return Err(format!("event subscribe rejected: {}", ack.trim()).into());
        }
        Ok(Self { reader })
    }

    /// Next relevant event; None when the socket dies.
    pub async fn next(&mut self) -> Option<WatchEvent> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.ok()?;
            if n == 0 {
                return None;
            }
            let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else { continue };
            // NOTE: wire names are dotted (same as subscription types).
            match ev["event"].as_str() {
                Some("pane.scroll_changed") => return Some(WatchEvent::Output),
                Some("pane.agent_status_changed") => return Some(WatchEvent::Status),
                _ => continue,
            }
        }
    }
}

/// Output produced after `base` — strips pre-existing scrollback so replies
/// contain only what happened since the last report.
pub fn delta<'a>(new: &'a [String], base: &[String]) -> &'a [String] {
    if base.is_empty() {
        return new;
    }
    for i in 0..new.len() {
        if new[i..].len() >= base.len() && new[i..i + base.len()] == *base {
            return &new[i + base.len()..];
        }
    }
    // Baseline scrolled off — cut after the newest baseline line still visible
    for b in base.iter().rev() {
        if b.trim().is_empty() {
            continue;
        }
        if let Some(pos) = new.iter().rposition(|l| l == b) {
            return &new[pos + 1..];
        }
    }
    new
}

pub fn join_trimmed(lines: &[String]) -> String {
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_delta_strips_baseline() {
        let base = v(&["old line 1", "old line 2"]);
        let new = v(&["old line 1", "old line 2", "fresh reply", "done"]);
        assert_eq!(delta(&new, &base), v(&["fresh reply", "done"]));
    }

    #[test]
    fn test_delta_no_baseline() {
        let new = v(&["a", "b"]);
        assert_eq!(delta(&new, &[]), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_delta_scrolled_off_fallback() {
        let base = v(&["marker", "tail"]);
        let new = v(&["junk", "marker", "new stuff"]);
        assert_eq!(delta(&new, &base), v(&["new stuff"]));
    }

    #[test]
    fn test_delta_nothing_new() {
        let base = v(&["same"]);
        let new = v(&["same"]);
        assert!(delta(&new, &base).is_empty());
    }

    #[test]
    fn test_join_trimmed() {
        assert_eq!(join_trimmed(&v(&["", "hi there", ""])), "hi there");
    }
}
