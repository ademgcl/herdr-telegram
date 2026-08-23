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

/// The agent's latest COMPLETED assistant reply for a project directory,
/// read straight from opencode's structured session store.
/// Returns None when opencode isn't present or has no reply yet.
pub fn final_reply(cwd: &str) -> Option<String> {
    let db = db_path()?;
    let sql = "SELECT coalesce(json_extract(p.data,'$.text'),'') AS txt \
        FROM part p \
        JOIN message m ON p.message_id=m.id \
        JOIN session s ON m.session_id=s.id \
        WHERE json_extract(m.data,'$.role')='assistant' \
          AND json_extract(p.data,'$.type')='text' \
          AND s.directory=?1 \
          AND p.message_id=( \
            SELECT m2.id FROM message m2 \
            JOIN part pf ON pf.message_id=m2.id \
              AND json_extract(pf.data,'$.type')='step-finish' \
            WHERE m2.session_id=m.session_id \
              AND json_extract(m2.data,'$.role')='assistant' \
            ORDER BY m2.time_created DESC LIMIT 1) \
        ORDER BY p.time_created ASC;";
    let out = Command::new("sqlite3")
        .arg("-json")
        .arg(&db)
        .arg(sql)
        .arg(cwd)
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
