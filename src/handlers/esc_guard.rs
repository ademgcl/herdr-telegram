//! Esc get_agent verdict: confirmed-gone vs transient blip.
//! Single source for every `/card` + `/esc` gate — a blip must retry
//! (HERDR_UNREACHABLE), never report UNKNOWN_TARGET via the gone path.

/// Post-RPC verdict for an esc/card gate.
#[derive(Debug, PartialEq)]
pub(crate) enum EscGate {
    Blocked,
    NotBlocked,
    Gone,
    Unreachable,
}

/// Pure verdict: `Ok(status)` gates on blocked; `Err(msg)` splits
/// classified death (`is_not_found` → Gone) from blips (Unreachable).
/// Fail-closed: unknown vocab is a blip, never confirmed death.
pub(crate) fn classify(res: Result<String, String>) -> EscGate {
    match res.as_deref() {
        Ok("blocked") => EscGate::Blocked,
        Ok(_) => EscGate::NotBlocked,
        Err(m) if crate::herdr::rpc::is_not_found(m) => EscGate::Gone,
        Err(_) => EscGate::Unreachable,
    }
}

#[cfg(test)]
#[path = "esc_guard_tests.rs"]
mod tests;
