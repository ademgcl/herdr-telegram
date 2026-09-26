use super::rpc::rpc;
use crate::types::{AgentDetail, AgentRow, Res};
use serde_json::json;

pub async fn list_agents(socket: &str) -> Res<Vec<AgentRow>> {
    let r = rpc(socket, "agent.list", json!({})).await?;
    // Fail-closed (list_panes parity): a missing `agents` key is a
    // malformed envelope, never "no agents" (the "all dead" signal).
    let arr = r
        .get("agents")
        .and_then(|v| v.as_array())
        .ok_or("agent.list returned no agents")?;
    let mut out = Vec::new();
    for a in arr {
        // Skip malformed rows: a ghost "?" pane would be subscribed
        // and reconciled downstream — fail-closed, never invent ids.
        let Some(pane) = a["pane_id"].as_str() else {
            continue;
        };
        out.push(AgentRow {
            kind: a["agent"].as_str().unwrap_or("?").into(),
            pane: pane.into(),
            title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
            status: a["agent_status"].as_str().unwrap_or("unknown").into(),
            ws: a["workspace_id"].as_str().unwrap_or("?").into(),
        });
    }
    Ok(out)
}

pub async fn get_agent(socket: &str, pane: &str) -> Res<AgentDetail> {
    let v = rpc(socket, "agent.get", json!({"target": pane})).await?;
    // Fail-closed (list_agents parity): a bare ack / missing `agent`
    // object is never "agent exists" — callers treat Ok as liveness
    // (focus moves, spawn verifies). Malformed rows must Err, never
    // invent "?" rows that mint success with ghost kinds.
    let a = v.get("agent").filter(|a| a.is_object()).ok_or("agent.get returned no agent")?;
    // Validated once (list_agents parity above): bind the id here so the
    // row below can never fall back to guessing the requested pane.
    let pid = a
        .get("pane_id")
        .and_then(|p| p.as_str())
        .ok_or("agent.get returned no pane_id")?;
    let cwd: String = a["foreground_cwd"]
        .as_str()
        .or(a["cwd"].as_str())
        .unwrap_or("")
        .into();
    let branch = derive_branch(a, &cwd).await;
    Ok(AgentDetail {
        kind: a["agent"].as_str().unwrap_or("?").into(),
        pane: pid.into(),
        title: a["terminal_title_stripped"].as_str().unwrap_or("").into(),
        status: a["agent_status"].as_str().unwrap_or("unknown").into(),
        ws: a["workspace_id"].as_str().unwrap_or("?").into(),
        cwd,
        branch,
    })
}

/// Pure parser for .git/HEAD content (e.g. `ref: refs/heads/main\n`).
pub fn parse_branch_from_head(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        let b = branch.trim();
        if !b.is_empty() {
            return Some(b.to_string());
        }
    } else if trimmed.len() >= 7 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let short = &trimmed[..7.min(trimmed.len())];
        return Some(format!("HEAD ({short})"));
    }
    None
}

/// `git branch --show-current`, ready to spawn: stdout piped so
/// `wait_with_output` actually captures the branch (an inherited stdout
/// reads back empty → the fallback never returns a name), stderr null so
/// non-git panes never append `fatal: not a git repository` to bot.log
/// (inherited stdio goes straight to the log, one line per card build).
fn git_branch_cmd(path: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-C", path, "branch", "--show-current"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Derive the git branch for an agent pane:
/// 1. Direct field in herdr response (`branch` or `git_branch`).
/// 2. Fast filesystem check on `.git/HEAD` or git worktree file.
/// 3. Subprocess fallback with timeout: `git -C <cwd> branch --show-current`.
///
/// Local inspection lives here (not via socket RPC) because herdr
/// exposes no branch API — it is read-only, 500ms-bounded, and never
/// mutates anything.
pub async fn derive_branch(a: &serde_json::Value, cwd: &str) -> Option<String> {
    if let Some(b) = a["branch"].as_str().or_else(|| a["git_branch"].as_str()) {
        let b = b.trim();
        if !b.is_empty() {
            return Some(b.to_string());
        }
    }

    let path = cwd.trim();
    if path.is_empty() {
        return None;
    }

    let cwd_path = std::path::Path::new(path);
    let git_entry = cwd_path.join(".git");
    if git_entry.is_dir() {
        if let Ok(content) = tokio::fs::read_to_string(git_entry.join("HEAD")).await
            && let Some(b) = parse_branch_from_head(&content)
        {
            return Some(b);
        }
    } else if git_entry.is_file()
        && let Ok(content) = tokio::fs::read_to_string(&git_entry).await
        && let Some(gitdir) = content.trim().strip_prefix("gitdir:")
        // Relative worktree gitdir resolves against the pane cwd, never
        // the bot CWD (else wrong HEAD → wrong branch card, silently).
        // Pane-controlled content: an absolute gitdir would discard the
        // cwd on join and read anywhere (`<abs>/HEAD`); `..` escapes the
        // same way — fall through to the bounded subprocess instead.
        && let gitdir = gitdir.trim()
        && !std::path::Path::new(gitdir).is_absolute()
        && !gitdir.split('/').any(|c| c == "..")
        && let Ok(head_content) =
            tokio::fs::read_to_string(cwd_path.join(gitdir).join("HEAD")).await
        && let Some(b) = parse_branch_from_head(&head_content)
    {
        return Some(b);
    }

    // Spawn (never `timeout(output())`): dropping an `output()` future
    // leaves the child running detached (`kill_on_drop(false)` default) —
    // a wedged git leaks one process per card build under exactly the
    // slowness that already hurts. `kill_on_drop(true)` reaps it.
    let child = git_branch_cmd(path).spawn().ok()?;
    let output = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        child.wait_with_output(),
    )
    .await
    .ok()?
    .ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    None
}

pub async fn read_agent_output(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "agent.read",
        json!({"target": pane, "source": "recent_unwrapped", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

/// Visible-viewport read: the ONLY source herdr serves while a pane is
/// blocked/working on an alternate screen (recent_unwrapped errors with
/// agent_not_idle there).
pub async fn read_agent_visible(socket: &str, pane: &str, lines: u32) -> Res<String> {
    let r = rpc(
        socket,
        "agent.read",
        json!({"target": pane, "source": "visible", "lines": lines}),
    )
    .await?;
    Ok(r["read"]["text"].as_str().unwrap_or("").trim().to_string())
}

pub async fn send_agent_keys(socket: &str, pane: &str, keys: &[&str]) -> Res<()> {
    rpc(
        socket,
        "agent.send_keys",
        json!({"target": pane, "keys": keys}),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
