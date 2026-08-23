use std::process::Command;

/// Path to opencode's session database, if present.
fn db_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("OPENCODE_DB") {
        return Some(p.into());
    }
    let home = std::env::var("HOME").ok()?;
    let p = std::path::PathBuf::from(format!("{home}/.local/share/opencode/opencode.db"));
    p.exists().then_some(p)
}

/// Assistant reply text produced AFTER `since_ms` for a project directory,
/// read straight from opencode's structured session store.
/// Only completed messages count (must contain a step-finish part).
pub fn final_reply(cwd: &str, since_ms: u64) -> Option<String> {
    let db = db_path()?;
    let sql = "SELECT coalesce(json_extract(p.data,'$.text'),'') AS txt \
        FROM part p \
        JOIN message m ON p.message_id=m.id \
        JOIN session s ON m.session_id=s.id \
        WHERE json_extract(m.data,'$.role')='assistant' \
          AND json_extract(p.data,'$.type')='text' \
          AND s.directory=?1 \
          AND p.time_created>=?2 \
          AND EXISTS(SELECT 1 FROM part pf WHERE pf.message_id=m.id \
              AND json_extract(pf.data,'$.type')='step-finish') \
        ORDER BY p.time_created ASC;";
    let out = Command::new("sqlite3")
        .arg("-json")
        .arg(&db)
        .arg(sql)
        .arg(cwd)
        .arg(since_ms.to_string())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    let text: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("txt").and_then(|v| v.as_str()))
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}
