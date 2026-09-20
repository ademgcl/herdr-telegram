use crate::types::Res;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

pub async fn rpc(socket: &str, method: &str, params: Value) -> Res<Value> {
    rpc_t(socket, method, params, 30).await
}

pub async fn rpc_t(socket: &str, method: &str, params: Value, timeout_secs: u64) -> Res<Value> {
    let fut = async {
        let mut conn = UnixStream::connect(socket).await?;
        let req = json!({"id": "tg", "method": method, "params": params});
        conn.write_all(format!("{req}\n").as_bytes()).await?;
        conn.flush().await?;
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        // Bounded like ctl (take-during-read): a rogue herdr line must
        // not OOM the bot. 2MB covers screens; anything larger refuses.
        {
            use tokio::io::AsyncReadExt;
            let mut limited = (&mut reader).take(2_097_152);
            limited.read_line(&mut line).await?;
            // Truncation, not size: take() caps the read, so a complete
            // line of exactly the cap (payload + newline) is legal — only
            // a missing trailing newline proves the reply was cut off.
            if line.len() >= 2_097_152 && !line.ends_with('\n') {
                return Err("herdr reply too large".into());
            }
        }
        if line.trim().is_empty() {
            return Err("herdr closed connection (empty reply)".into());
        }
        let v: Value = serde_json::from_str(line.trim())?;
        // Explicit `"error": null` is a SUCCESS envelope (some calls
        // include it); only a non-null error fails. A missing `result`
        // stays silent Null: fire-and-forget calls (prompt submit, keys,
        // close, rename) ignore the value, and failing them on a bare
        // ack would break working submits.
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            // Object-form (`{"message": ...}`) or string-form errors both
            // passthrough (string-form must not collapse to generic or
            // `contains("blocked")` dispatch misses the blocked-card path).
            let msg = match e {
                Value::String(s) => s.clone(),
                _ => e["message"].as_str().unwrap_or("herdr error").to_string(),
            };
            return Err(msg.into());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    };
    tokio::time::timeout(Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| format!("herdr {method} timed out"))?
}

pub async fn ping(socket: &str) -> Res<Value> {
    rpc(socket, "ping", json!({})).await
}

/// Herdr "gone" classifier: confirmed-dead pane vs transient blip.
/// Single source — dm_typewait + reconcile duplicated these and re-drifted.
/// Pure for tests. Fail-closed: unknown vocab is a blip (keep + retry).
/// No bare `"no agent"` arm by design: it over-matches transient read
/// errors (`no agent output…`) into confirmed death (topic close + wipe).
/// Underscore shapes (`agent_not_found`) already match via `not_found`.
pub fn is_not_found(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("not found")
        || m.contains("not_found")
        || m.contains("notfound")
        || m.contains("no such")
        || m.contains("no_such")
        || m.contains("unknown pane")
        || m.contains("unknown_pane")
}
/// Timeout classifier (single source): herdr timeout wording varies in
/// case (`timed out` / `Timed out`), so match case-insensitively. Used by
/// the visible-fallback guards below — a missed match doubles a
/// sick-herdr read to ~60s on the path budgeted to stay ~30s.
pub fn is_timeout(msg: &str) -> bool {
    msg.to_lowercase().contains("timed out")
}
/// Alternate-screen fallback verdict (single source): blocked/working
/// panes reject `recent_unwrapped`, so readers fall back to `visible`.
/// Confirmed death must never fall back (wrong-pane output) and timeouts
/// must not double the sick-herdr RPC budget. Pure for tests.
pub fn should_fallback_visible(msg: &str) -> bool {
    !is_not_found(msg) && !is_timeout(msg)
}

/// Agent-lookup retry verdict (single source for shell-provision):
/// only a confirmed-dead lookup reads as shell — any blip
/// (timeout/unreachable) retries visibly instead of injecting shell
/// text into live agent work. Pure for tests.
pub fn should_retry_agent_lookup(msg: &str) -> bool {
    !is_not_found(msg)
}
/// Event-subscribe ack rejection: only a parsed non-null `error`
/// rejects. Success acks may carry `"error": null`, which a substring
/// match misreads as rejection (tight resubscribe loop). Unparseable
/// acks stay lenient (proceed) — same as the old substring check.
pub fn ack_rejected(ack: &str) -> bool {
    serde_json::from_str::<Value>(ack.trim())
        .ok()
        .and_then(|v| v.get("error").cloned())
        .map(|e| !e.is_null())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_not_found_variants_vs_blips() {
        assert!(is_not_found("agent_not_found"));
        assert!(is_not_found("Agent not found"));
        assert!(is_not_found("no such pane"));
        assert!(is_not_found("unknown_pane w1:p9"));
        assert!(is_not_found("notfound"));
        // Bare "no agent" is a blip by design (fail-closed): transient
        // read errors containing it must retry, never confirm death.
        assert!(!is_not_found("no agent here"));
        assert!(!is_not_found("herdr unreachable — try again"));
        assert!(!is_not_found("timed out"));
    }

    #[test]
    fn test_should_fallback_visible_death_and_timeout_never() {
        // Alternate-screen fallback only: confirmed death would serve
        // wrong-pane output, timeouts would double the sick-herdr budget.
        // Timeout matches case-insensitively (herdr capitalizes it).
        assert!(should_fallback_visible("herdr unreachable — try again"));
        assert!(should_fallback_visible("no agent output yet"));
        assert!(!should_fallback_visible("agent_not_found"));
        assert!(!should_fallback_visible("unknown_pane w1:p9"));
        assert!(!should_fallback_visible("herdr agent.prompt timed out"));
        assert!(!should_fallback_visible("herdr agent.prompt Timed out"));
        assert!(is_timeout("Timed Out waiting for pane"));
        assert!(!is_timeout("herdr unreachable"));
    }

    #[test]
    fn test_should_retry_agent_lookup_only_confirmed_dead_proceeds() {
        // Blips retry visibly; only confirmed death reads as shell.
        assert!(should_retry_agent_lookup("herdr unreachable — try again"));
        assert!(should_retry_agent_lookup("herdr timed out"));
        assert!(!should_retry_agent_lookup("agent_not_found"));
        assert!(!should_retry_agent_lookup("no such pane"));
    }
}
