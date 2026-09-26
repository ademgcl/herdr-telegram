use super::arbitrate::STREAM_MIN_CHARS;
use crate::types::Res;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// Events a prompt watcher cares about, pushed by herdr.
pub enum WatchEvent {
    /// Pane produced new output (scroll snapshot changed).
    Output,
    /// Agent semantic state transitioned.
    Status,
}

/// Persistent per-pane subscription to `pane.scroll_changed` and
/// `pane.agent_status_changed` on its own unix socket. Dials directly
/// (not via `herdr::rpc`) on purpose: `rpc()` consumes the connection
/// for one request/reply, while this needs the split halves held open
/// for a long-lived subscription + ack check.
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
        // Bounded (ctl parity, take-during-read): a hung/rogue ack must
        // not OOM the watcher. Events are small — 64KB cap.
        {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt};
            let mut limited = (&mut reader).take(65_536);
            limited.read_line(&mut ack).await?;
            // At-cap AND unterminated means truncated: a complete line of
            // exactly cap bytes (newline included) is not oversize.
            if ack.len() >= 65_536 && !ack.ends_with('\n') {
                return Err("event subscribe ack too large".into());
            }
        }
        if crate::herdr::client::ack_rejected(&ack) {
            return Err(format!("event subscribe rejected: {}", ack.trim()).into());
        }
        Ok(Self { reader })
    }

    /// Open with a bound: a hung ack must not freeze the watcher
    /// pre-select (no settle checks, no cancel) — degrade to polling.
    pub async fn open_bounded(socket: &str, pane: &str, secs: u64) -> Res<Self> {
        match tokio::time::timeout(
            std::time::Duration::from_secs(secs),
            Self::open(socket, pane),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err("event subscribe ack timed out".into()),
        }
    }

    /// Next relevant event; None when the socket dies.
    pub async fn next(&mut self) -> Option<WatchEvent> {
        loop {
            let mut line = String::new();
            // Bounded: a rogue herdr line must not OOM a watcher (ctl
            // take-during-read parity). 256KB covers events; larger
            // skips instead of buffering unbounded.
            {
                use tokio::io::{AsyncBufReadExt, AsyncReadExt};
                let mut limited = (&mut self.reader).take(262_144);
                let n = limited.read_line(&mut line).await.ok()?;
                // At-cap AND unterminated means truncated: a complete
                // line of exactly cap bytes (newline included) is a full
                // event — draining past it would eat the next event line.
                if line.len() >= 262_144 && !line.ends_with('\n') {
                    // `take` cut mid-line: the tail would else parse as the
                    // next event. Drain to the newline first (bounded, never
                    // OOM), then skip — events-task drain parity.
                    drop(limited);
                    use tokio::io::AsyncReadExt as _DrainExt;
                    let mut tail = Vec::new();
                    let _ = (&mut self.reader)
                        .take(4_194_304)
                        .read_until(b'\n', &mut tail)
                        .await;
                    continue;
                }
                if n == 0 {
                    return None;
                }
            }
            let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
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
    // Narrower-window re-read: anchors and readers use different widths
    // (finals anchor wide, the notifier reads a short tail), so `new` can
    // be a strict tail of `base`. The contiguous search below can never
    // match then (base is longer than new) and the fallback misfires on
    // footer drift, reposting the whole answer as "fresh" (duplicate
    // final). Longest base-suffix == new-prefix overlap is exact here:
    // same source, tail window, no new output means full overlap.
    if new.len() < base.len() {
        for k in (1..=new.len()).rev() {
            if new[..k] == base[base.len() - k..] {
                return &new[k..];
            }
        }
        return new;
    }
    for i in 0..new.len() {
        if new[i..].len() >= base.len() && new[i..i + base.len()] == *base {
            return &new[i + base.len()..];
        }
    }
    // Baseline scrolled off — cut after the newest baseline line still visible.
    // First occurrence, never last: with duplicated lines the cut position is
    // ambiguous, and cutting after the last match silently drops fresh output
    // (loss) while cutting after the first only risks duplicating old lines
    // (noise). Duplicates-over-silence: never lose the reply.
    for b in base.iter().rev() {
        if b.trim().is_empty() {
            continue;
        }
        if let Some(pos) = new.iter().position(|l| l == b) {
            return &new[pos + 1..];
        }
    }
    new
}

/// Stale re-extraction verdict (pure, tested): every content line of
/// `body` already present in the anchored `base` means delta
/// misalignment re-served delivered text — the status-bar token counts
/// churn every tick, defeating exact overlap, so the fallback cut lands
/// early and the just-delivered final buzzes twice. Never re-buzz it;
/// callers consume + anchor past it instead. Genuine fresh output always
/// carries a line the baseline never saw (a verbatim repeat is
/// indistinguishable from no-change — accepted). Small bodies post
/// freely (STREAM_MIN_CHARS parity: short replies like "ok"/"done" must
/// never die on a coincidental scrollback match). Empty base never
/// matches (first sight always posts).
pub fn is_stale_body(body: &str, base: &[String]) -> bool {
    if base.is_empty() || body.chars().count() < STREAM_MIN_CHARS {
        return false;
    }
    let mut content = 0;
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        content += 1;
        if !base.iter().any(|b| b.trim() == t) {
            return false;
        }
    }
    content > 0
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
    fn test_delta_fallback_keeps_dup_lines() {
        // Duplicated lines make the cut ambiguous: never drop fresh output
        // (first occurrence) — a duplicate old line is only noise.
        let base = v(&["x", "y"]);
        let new = v(&["y", "y", "fresh"]);
        assert_eq!(delta(&new, &base), v(&["y", "fresh"]));
    }

    #[test]
    fn test_delta_nothing_new() {
        let base = v(&["same"]);
        let new = v(&["same"]);
        assert!(delta(&new, &base).is_empty());
    }

    #[test]
    fn test_delta_narrower_tail_of_wide_base_is_empty() {
        // Final anchors 200 lines, notifier re-reads an 80-line tail of
        // the same screen: no new output must read as empty, never as
        // a "fresh" duplicate of the just-delivered final.
        let base = v(&["a", "b", "c", "d"]);
        let new = v(&["c", "d"]);
        assert!(delta(&new, &base).is_empty());
    }

    #[test]
    fn test_delta_narrower_tail_returns_only_fresh() {
        let base = v(&["a", "b", "c"]);
        let new = v(&["b", "c", "fresh"]);
        assert_eq!(delta(&new, &base), v(&["fresh"]));
    }

    #[test]
    fn test_delta_narrower_without_overlap_returns_all() {
        // Reminted screen sharing nothing: whole window is fresh.
        let base = v(&["a", "b", "c"]);
        let new = v(&["x", "y"]);
        assert_eq!(delta(&new, &base), v(&["x", "y"]));
    }

    #[test]
    fn test_join_trimmed() {
        assert_eq!(join_trimmed(&v(&["", "hi there", ""])), "hi there");
    }

    #[test]
    fn test_stale_body_dup_shape_suppresses() {
        // Delivered reply re-extracted after footer churn: every content
        // line sits in the anchored baseline (the observed duplicate).
        let base = v(&[
            "old scroll",
            "The guard was missing — fixed.",
            "Plus a second reply line for size.",
        ]);
        let body = "The guard was missing — fixed.\nPlus a second reply line for size.";
        assert!(is_stale_body(body, &base));
    }

    #[test]
    fn test_stale_body_fresh_line_posts() {
        let base = v(&["old scroll", "prior reply"]);
        assert!(!is_stale_body(
            "prior reply plus a brand new output line here",
            &base
        ));
    }

    #[test]
    fn test_stale_body_first_sight_shorts_and_blanks_post() {
        assert!(!is_stale_body("anything at all here, please", &[]));
        assert!(!is_stale_body("ok", &v(&["ok", "scrollback"])));
        assert!(!is_stale_body("", &v(&["x"])));
        assert!(!is_stale_body("  \n ", &v(&["x"])));
    }
}
