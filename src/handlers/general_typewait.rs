//! General-topic typewait shell-flip probe (split from `general`:
//! 300-line file limit).
use crate::state::AppState;

/// Shell-flip probe verdict (pure, tested): classifies the
/// get_agent + list_panes reads so General/forum/DM typewaits can never
/// drift. `Proceed` types; `StaleShell` evicts + refuses (agent→shell
/// flip); `DeadPane` evicts + degrades to normal routing (never a type
/// attempt); `Unreachable` keeps the waiter + refuses visibly
/// (fail-closed: ambiguous reads never consume).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypewaitProbe {
    Proceed,
    StaleShell,
    DeadPane,
    Unreachable,
}

pub(crate) fn classify_typewait_probe(
    get_agent_ok: bool,
    is_not_found: bool,
    list_ok: bool,
    list_contains: bool,
) -> TypewaitProbe {
    if get_agent_ok {
        return TypewaitProbe::Proceed;
    }
    if !is_not_found {
        return TypewaitProbe::Unreachable;
    }
    if !list_ok {
        return TypewaitProbe::Unreachable;
    }
    if list_contains {
        TypewaitProbe::StaleShell
    } else {
        TypewaitProbe::DeadPane
    }
}

/// Probe outcome: `Handled` (caller returns), `Degraded` (caller falls
/// through to normal routing — waiter already evicted, never a type
/// attempt), `Proceed` (caller types into the live agent).
pub(crate) enum ProbeOut {
    Proceed,
    Degraded,
    Handled,
}

/// Shell-flip parity with forum/dm typewait: a waiter stranded across
/// an agent→shell flip must not eat the next message as typed input —
/// drop it with STALE_TYPEWAIT_SHELL instead of typing into a shell.
/// Fail-closed: ambiguous reads keep the waiter + refuse, never consume
/// on a blip. A dead pane degrades to normal routing below (never a
/// type attempt). Single source: `classify_typewait_probe` (parity with
/// forum/dm arms — the reads below only feed it).
pub(crate) async fn probe_typewait(
    s: &AppState,
    chat: i64,
    thread_id: Option<i64>,
    wpane: &str,
) -> ProbeOut {
    let probe = match crate::herdr::client::get_agent(&s.cfg.socket, wpane).await {
        Ok(_) => TypewaitProbe::Proceed,
        Err(e) => {
            let not_found = crate::herdr::rpc::is_not_found(&e.to_string());
            if !not_found {
                TypewaitProbe::Unreachable
            } else {
                match crate::herdr::client::list_panes(&s.cfg.socket).await {
                    Ok(l) => {
                        classify_typewait_probe(false, true, true, l.iter().any(|p| p == wpane))
                    }
                    Err(_) => TypewaitProbe::Unreachable,
                }
            }
        }
    };
    match probe {
        TypewaitProbe::Proceed => ProbeOut::Proceed,
        TypewaitProbe::Unreachable => {
            s.tg.send_msg(chat, thread_id, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            ProbeOut::Handled
        }
        TypewaitProbe::StaleShell => {
            // Normalized (forum::waiter_key parity): General None and
            // Some(1) are one conversation — a raw remove would leave
            // the armed key behind to brick later input.
            s.typewait
                .lock()
                .await
                .remove(&super::forum::waiter_key(chat, thread_id));
            s.tg.send_msg(chat, thread_id, crate::ui::STALE_TYPEWAIT_SHELL, None)
                .await;
            ProbeOut::Handled
        }
        TypewaitProbe::DeadPane => {
            s.typewait
                .lock()
                .await
                .remove(&super::forum::waiter_key(chat, thread_id));
            ProbeOut::Degraded
        }
    }
}
