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
        reader.read_line(&mut line).await?;
        let v: Value = serde_json::from_str(line.trim())?;
        if let Some(e) = v.get("error") {
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
