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

/// Auto label (`space-N`) when `/space` gets no name: first unused N,
/// not len+1 (deleted middle spaces must not collide). None on an
/// unreadable list (fail-closed): guessing `space-1` on an outage
/// mints a duplicate label — the caller refuses instead.
pub async fn next_label(s: &AppState) -> Option<String> {
    let labels: Vec<String> = list_workspaces(&s.cfg.socket)
        .await
        .ok()?
        .into_iter()
        .map(|w| w.label)
        .collect();
    let mut n = 1;
    while labels.iter().any(|l| l == &format!("space-{n}")) {
        n += 1;
    }
    Some(format!("space-{n}"))
}

/// Resolve a workspace id-or-label to its id: buttons pass ids, humans
/// type labels (`/spawn opencode space-1`). Unknown → None.
pub async fn resolve_ws(s: &AppState, spec: &str) -> Option<String> {
    list_workspaces(&s.cfg.socket)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.id == spec || w.label == spec)
        .map(|w| w.id)
}

pub async fn open_space(s: &AppState, chat: i64, thread: Option<i64>, label: &str) {
    // create_workspace fail-closed (empty→Err): no dead Ok-empty arm.
    let ws_id = match create_workspace(&s.cfg.socket, label).await {
        Ok(id) => id,
        Err(e) => {
            s.tg.send_msg(
                chat,
                thread,
                &format!(
                    "⚠️ failed to create space `{label}`: {}",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
            return;
        }
    };
    super::shell::open_space_shell(s, chat, thread, &ws_id).await;
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
