use super::target::resolve_target;
use crate::{jobs::enqueue_prompt, state::AppState, types::AgentRow};

/// Pane-id shape (`w1:p1`, dead `w9:p7`): `w<n>` colon `p<n>` suffix,
/// no URL/mention chars. Pure for tests — ordinary words (`note:`,
/// `note:p1`, `https://…`) return false so normal prompts never refuse.
pub(crate) fn pane_shaped(head: &str) -> bool {
    let Some((a, b)) = head.split_once(':') else {
        return false;
    };
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if head.contains('/') || head.contains('@') || head.contains('.') {
        return false;
    }
    if a.contains(char::is_whitespace) || b.contains(char::is_whitespace) {
        return false;
    }
    // Real panes are `w<n>:p<n>` — the `w` prefix reclaims `note:p1`
    // style prompts that a suffix-only check would refuse. Full-digit
    // match both sides so `w1x:p1`/`w1:p1extra` stay prompts.
    let mut ac = a.chars();
    if ac.next() != Some('w') {
        return false;
    }
    let arest: String = ac.collect();
    if arest.is_empty() || !arest.chars().all(|d| d.is_ascii_digit()) {
        return false;
    }
    // Suffix `p<n>` covers live + dead panes (`w1:p1`, `w9:p7`) —
    // full-digit match so `w1:p1extra` stays a prompt, never an address.
    let mut c = b.chars();
    if c.next() != Some('p') {
        return false;
    }
    let rest: String = c.collect();
    !rest.is_empty() && rest.chars().all(|d| d.is_ascii_digit())
}

/// Trailing punctuation (`w8:p1, fix`) must not turn an address into
/// prompt text for the wrong session. Pure for tests. Never returns ""
/// (bare `...`/`!!!` stays prompt text, never the sole-agent address).
pub(crate) fn strip_addr_punct(head: &str) -> &str {
    let h = head.trim_end_matches([',', '.', ';', '!', '?', ':']);
    if h.is_empty() { head } else { h }
}

/// Bare text prompt routing: explicit `<pane> <prompt>`, else reply,
/// else focus, else sole agent; shell-pane fallback when the target is
/// rowless. Blocked panes get typed input, others enqueue a prompt job.
pub(crate) async fn handle_bare_prompt(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    text: &str,
    reply_pane: Option<String>,
) {
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    // Stripped form keeps punctuated pane ids refusing instead of misrouting.
    let head_addr = strip_addr_punct(head);
    // A bare pane id (or kind) with no prompt is an incomplete address,
    // never prompt text: enqueuing the literal id into focus/sole-agent
    // sends "w8:p1" to the wrong session as a prompt. Refuse with usage.
    // Stripped branch is pane-shaped only: `!`/`?`/`,` are normal prompt
    // punctuation, so bare `opencode!` stays a prompt — only a punctuated
    // pane id (`w8:p1,`) is unambiguously an address, never text.
    if rest.trim().is_empty()
        && (resolve_target(rows, Some(head)).is_some()
            || (head_addr != head
                && pane_shaped(head_addr)
                && resolve_target(rows, Some(head_addr)).is_some()))
    {
        s.tg.send_msg(
            chat,
            None,
            "usage: `<pane> <prompt>` — name a pane and a prompt",
            None,
        )
        .await;
        return;
    }
    // Dead/ambiguous address is never prompt text: a dead pane id
    // (`w1:p9 fix bug`) or an ambiguous kind (`opencode fix` with two)
    // refuses with UNKNOWN_TARGET instead of prompting focus/sole-agent
    // with the address as text (fail-closed parity with dm_info.rs:30).
    // `pane_shaped` keeps ordinary `note:`/`https://` prompts serving.
    // Explicit rowless shells (`w8:p7 ls`) serve via the shell fallback
    // (/keys parity): liveness probe first, corpse refuses, outage retries.
    if resolve_target(rows, Some(head)).is_none()
        && resolve_target(rows, Some(head_addr)).is_none()
        && (pane_shaped(head)
            || pane_shaped(head_addr)
            || rows.iter().any(|r| r.kind == head || r.kind == head_addr))
    {
        // Kinds never contain ':' so pane_shaped implies not-a-kind —
        // no extra kind conjunct (zero-dead-code).
        let shell_head = if pane_shaped(head) {
            head
        } else if pane_shaped(head_addr) {
            head_addr
        } else {
            ""
        };
        if !shell_head.is_empty() && !rest.trim().is_empty() {
            if super::shell::probe_shell_live(s, chat, None, shell_head).await {
                super::shell::run_shell_fallback(s, chat, shell_head.to_string(), rest).await;
            }
            return;
        }
        s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
            .await;
        return;
    }
    let explicit = if rest.is_empty() {
        None
    } else {
        // Stripped fallback is pane-shaped only: punctuated kinds
        // (`opencode, fix`) stay prompt text via reply/focus/sole-agent
        // instead of misrouting into the wrong session (fail-closed).
        resolve_target(rows, Some(head))
            .or_else(|| {
                if head_addr != head && pane_shaped(head_addr) {
                    resolve_target(rows, Some(head_addr))
                } else {
                    None
                }
            })
            .map(|r| (r, rest.to_string()))
    };
    let via_reply = reply_pane
        .as_deref()
        .and_then(|p| rows.iter().find(|r| r.pane == p))
        .cloned();
    // Single focus read (no TOCTOU): a concurrent set_focus between
    // two reads lost just-set live focus into sole/fallback misroute.
    let focus = s.get_focus().await;
    let via_focus = focus
        .as_deref()
        .and_then(|p| rows.iter().find(|r| r.pane == p))
        .cloned();

    let (row, prompt_text) = if let Some(pair) = explicit {
        pair
    } else if let Some(r) = via_reply {
        (r, text.to_string())
    } else if let Some(rp) = reply_pane.clone() {
        // The reply names a rowless (shell) pane: run it as a command
        // instead of falling through to the focused agent (which would
        // send shell text to the wrong agent as a prompt). Live arm, not
        // dead code: shells never appear in rows, so via_reply is always
        // None for them and only this branch can serve them. Fail-closed
        // first: a corpse reply (dead pane) refuses with UNKNOWN_TARGET,
        // an unreadable pane list refuses with HERDR_UNREACHABLE (tap
        // parity) — rowless LIVE shells still serve via the fallback.
        // (`via_reply==None` here already implies unmatched, so no
        // extra dead-check — just the shared liveness probe.)
        if !super::shell::probe_shell_live(s, chat, None, &rp).await {
            return;
        }
        super::shell::run_shell_fallback(s, chat, rp, text).await;
        return;
    } else if let Some(r) = via_focus {
        (r, text.to_string())
    } else if let Some(focus) = focus.filter(|f| rows.iter().all(|r| r.pane != *f)) {
        // Rowless focus (live shell or corpse) with no reply: the sole
        // agent below must not shadow it — shell text into the agent is
        // a cross-session write. Liveness probe first (reply-corpse
        // parity): a corpse refuses UNKNOWN_TARGET; an unreadable list
        // refuses HERDR_UNREACHABLE (never masks outage as "gone").
        // Pass the snapshot (no re-read inside the fallback): a concurrent
        // set_focus between reads must not reroute shell text as a prompt.
        if !super::shell::probe_shell_live(s, chat, None, &focus).await {
            return;
        }
        super::shell::run_shell_fallback(s, chat, focus, text).await;
        return;
    } else if let Some(r) = resolve_target(rows, Some("")) {
        // Sole agent: explicit already consumed any `<pane> <prompt>`
        // address above, so the full text is the prompt (no prefix
        // strip — the old strip was unreachable dead code).
        (r, text.to_string())
    } else {
        // No agent rows at all: reply may still name a rowless shell.
        // Never re-read focus here (snapshot was None): a concurrent
        // set_focus between reads must not reroute shell text as a prompt.
        if let Some(rp) = reply_pane.clone() {
            if super::shell::probe_shell_live(s, chat, None, &rp).await {
                super::shell::run_shell_fallback(s, chat, rp, text).await;
            }
        } else {
            s.tg.send_msg(chat, None, crate::ui::UNKNOWN_TARGET, None)
                .await;
        }
        return;
    };

    // Blocked panes reject text prompts — type into the waiting prompt.
    // Focus follows success only.
    if row.status == "blocked" {
        match super::tap::type_text(s, &row.pane, &prompt_text).await {
            Ok(()) => {
                s.set_focus(&row.pane).await;
                s.tg.send_silent(chat, None, &crate::ui::typed_ack(&row.pane))
                    .await;
                // Resumed work owns no job — follow it to the final reply.
                crate::jobs::follow::follow_answer(s, &row.pane, chat, None, &prompt_text).await;
            }
            // Resumed between snapshot and send: the text becomes a
            // regular prompt instead of stray input.
            Err(super::tap::TypeError::Resumed) => {
                enqueue_prompt(
                    crate::state::AppState::clone(s),
                    chat,
                    None,
                    row,
                    prompt_text,
                )
                .await;
            }
            Err(e) => {
                // Same why-plus-card rule as topics: the reason always
                // shows, then fresh buttons (or text fallback).
                if s.block_held(&row.pane).await {
                    s.tg.send_msg(chat, None, crate::ui::ANSWER_IN_FLIGHT, None)
                        .await;
                } else {
                    s.tg.send_msg(
                        chat,
                        None,
                        &format!(
                            "⚠️ type failed: {}",
                            crate::types::mask_home(&e.to_string())
                        ),
                        None,
                    )
                    .await;
                    if !super::dialog::send_blocked_card(s, chat, None, &row.pane).await {
                        s.tg.send_msg(chat, None, crate::ui::CARD_FAILED_PC, None)
                            .await;
                    }
                }
            }
        }
        return;
    }
    // No pre-focus: enqueue sets focus after a live deliver.
    enqueue_prompt(
        crate::state::AppState::clone(s),
        chat,
        None,
        row,
        prompt_text,
    )
    .await;
}

#[cfg(test)]
#[path = "dm_prompt_tests.rs"]
mod tests;
