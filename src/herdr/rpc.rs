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
            if line.len() >= 2_097_152 {
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
            return Err(e["message"]
                .as_str()
                .unwrap_or("herdr error")
                .to_string()
                .into());
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
