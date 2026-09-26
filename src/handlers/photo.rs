//! Incoming photo routing: download-then-prompt for agent surfaces,
//! visible hints everywhere else (a photo must never vanish silently).
//! Split from the surface handlers (300-line file limit).
use crate::state::AppState;

/// Prompt marker for a fetched image (single source — DM/topic-agent
/// compose it identically so the agent always sees the same shape).
/// Absolute path: agents resolve relative to their own cwd, never the
/// bot's.
pub fn photo_marker(path: &std::path::Path) -> String {
    format!("\n\n[attached image: {}]", path.display())
}

/// Command-caption verdict (pure, tested): a caption opening with `/`
/// is a command, never a prompt — downloading the image for it would
/// burn up to ~45s of the sequential pump just to discard it at the
/// command arm. Commands serve imageless (the result confirms receipt).
pub fn is_command_caption(caption: &str) -> bool {
    caption
        .split_whitespace()
        .next()
        .is_some_and(|w| w.starts_with('/'))
}

/// Compose caption + marker (pure, tested): caption-less photos prompt
/// with the marker alone (the model describes what it sees) — the
/// prompt flow from here is identical to text.
pub fn compose_prompt(caption: &str, marker: &str) -> String {
    if caption.trim().is_empty() {
        marker.to_string()
    } else {
        format!("{caption}{marker}")
    }
}

/// Fetch-or-refuse (single source for DM/topic-agent): `Some(prompt)`
/// to serve, `None` after visibly refusing a failed fetch (serving
/// caption-only would silently drop the image the user asked about —
/// fail-closed, no half prompt).
pub async fn fetch_prompt(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    file_id: &str,
    caption: &str,
) -> Option<String> {
    match crate::telegram::photo::fetch_photo(s, file_id).await {
        Some(path) => Some(compose_prompt(caption, &photo_marker(&path))),
        None => {
            s.tg.send_msg(chat, thread, crate::ui::PHOTO_FETCH_FAILED, None)
                .await;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compose_prompt_marker_only_without_caption() {
        assert_eq!(compose_prompt("", "[m]"), "[m]");
        assert_eq!(compose_prompt("   ", "[m]"), "[m]");
        assert_eq!(compose_prompt("look", "[m]"), "look[m]");
    }

    #[test]
    fn test_photo_marker_carries_absolute_path() {
        let m = photo_marker(std::path::Path::new("/tmp/x/img/photo-1.jpg"));
        assert!(m.contains("/tmp/x/img/photo-1.jpg"));
        assert!(m.contains("[attached image:"));
    }

    #[test]
    fn test_is_command_caption_slash_first_word() {
        assert!(is_command_caption("/reset"));
        assert!(is_command_caption("/model@Bot x"));
        assert!(!is_command_caption("look at this"));
        assert!(!is_command_caption(""));
        assert!(!is_command_caption("a/b test"));
    }
}
