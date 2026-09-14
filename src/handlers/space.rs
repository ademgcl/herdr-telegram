/// `/space <name>`: create a workspace, then open a shell pane + forum
/// topic mapped to it so the user keeps chatting in that topic.
/// (Replaces the old bare `/newspace`, which created the space but left
/// the user in General with nowhere to talk.)
use crate::{
    herdr::client::{create_workspace, list_workspaces},
    state::AppState,
};

/// Label must be non-blank; blank input auto-labels (`space-N`).
pub fn check_label(label: &str) -> bool {
    !label.trim().is_empty()
}

/// Auto label (`space-N`) when `/space` gets no name.
pub async fn next_label(s: &AppState) -> String {
    let n = list_workspaces(&s.cfg.socket).await.map(|w| w.len()).unwrap_or(0) + 1;
    format!("space-{n}")
}

pub async fn open_space(s: &AppState, chat: i64, thread: Option<i64>, label: &str) {
    let ws_id = match create_workspace(&s.cfg.socket, label).await {
        Ok(id) => id,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ failed to create space `{label}`: {e}"), None).await;
            return;
        }
    };
    super::shell::open_shell(s, chat, thread, Some(&ws_id)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_label() {
        assert!(check_label("work"));
        assert!(!check_label(""));
        assert!(!check_label("   "));
    }
}
