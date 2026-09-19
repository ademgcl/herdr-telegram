use super::target::{resolve_target, unmatched_reply};
use crate::{state::AppState, types::AgentRow};

pub(crate) async fn handle_model(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    // `/model [target] [search]` — first token is a target only when it
    // resolves to a pane/kind; otherwise the whole arg is the search.
    // Pane-shaped (`w<n>:p<n>`) or known-kind tokens are never search
    // text: an unresolvable one is an explicit address — refuse instead
    // of falling through to focus/sole with a junk filter driving the
    // wrong picker's keys. Strict shape only (`note:fix`, URLs stay searches).
    let (mut pane, query) = match arg.split_once(char::is_whitespace) {
        Some((t, rest)) => match resolve_target(rows, Some(t)) {
            Some(r) => (Some(r.pane), rest.trim()),
            None if super::dm_prompt::pane_shaped(t) || rows.iter().any(|r| r.kind == t) => {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
                return;
            }
            None => (None, arg),
        },
        None => {
            if arg.is_empty() {
                (None, "")
            } else if let Some(r) = resolve_target(rows, Some(arg)) {
                (Some(r.pane), "")
            } else if super::dm_prompt::pane_shaped(arg) || rows.iter().any(|r| r.kind == arg) {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
                return;
            } else {
                (None, arg)
            }
        }
    };
    if pane.is_none()
        && let Some(p) = reply_pane
            .as_deref()
            .and_then(|p| rows.iter().find(|r| r.pane == p))
    {
        pane = Some(p.pane.clone());
    }
    // Corpse reply: refuse — falling through would drive the model
    // picker key sequence into the wrong live session's work.
    if pane.is_none() && unmatched_reply(rows, reply_pane) {
        s.tg.send_msg(
            chat,
            None,
            "who? `/model <pane>` or tap an agent in /agents",
            None,
        )
        .await;
        return;
    }
    if pane.is_none() {
        // Rowless focus must not shadow to sole-agent (wrong picker keys).
        match s.get_focus().await {
            Some(f) if rows.iter().any(|r| r.pane == f) => {
                pane = Some(f);
            }
            Some(_) => {
                s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                    .await;
                return;
            }
            None => {
                pane = resolve_target(rows, Some("")).map(|r| r.pane);
            }
        }
    }
    let Some(pane) = pane else {
        s.tg.send_msg(
            chat,
            None,
            "who? `/model <pane>` or tap an agent in /agents",
            None,
        )
        .await;
        return;
    };
    if query.is_empty() {
        super::model::show_model(s, chat, None, &pane).await;
    } else {
        let filter = super::model::search_filter(query);
        super::model::switch_by_filter(s, chat, None, &pane, &filter, query).await;
    }
}
