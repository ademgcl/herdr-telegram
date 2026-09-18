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
    // Pane-shaped (`w:p`) tokens are never search text: an unresolvable
    // one is an explicit address — refuse instead of falling through to
    // focus/sole with a junk filter driving the wrong picker's keys.
    let (mut pane, query) = match arg.split_once(char::is_whitespace) {
        Some((t, rest)) => match resolve_target(rows, Some(t)) {
            Some(r) => (Some(r.pane), rest.trim()),
            None if t.contains(':') => {
                s.tg.send_msg(chat, None, "unknown target — see /agents", None)
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
            } else if arg.contains(':') {
                s.tg.send_msg(chat, None, "unknown target — see /agents", None)
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
    if pane.is_none()
        && let Some(f) = s.get_focus().await
        && rows.iter().any(|r| r.pane == f)
    {
        pane = Some(f);
    }
    if pane.is_none() {
        pane = resolve_target(rows, Some("")).map(|r| r.pane);
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
