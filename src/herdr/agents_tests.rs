//! Tests for [`super::agents`] (split: 300-line file limit).
use super::*;

#[test]
fn test_parse_branch_from_head() {
    assert_eq!(
        parse_branch_from_head("ref: refs/heads/main\n"),
        Some("main".to_string())
    );
    assert_eq!(
        parse_branch_from_head("ref: refs/heads/feature/d7-branch"),
        Some("feature/d7-branch".to_string())
    );
    assert_eq!(
        parse_branch_from_head("3425da8982341234567890"),
        Some("HEAD (3425da8)".to_string())
    );
    assert_eq!(parse_branch_from_head(""), None);
    assert_eq!(parse_branch_from_head("invalid head"), None);
}

#[tokio::test]
async fn test_derive_branch_from_json() {
    let a = json!({"branch": "feat-json"});
    assert_eq!(derive_branch(&a, "").await, Some("feat-json".to_string()));
    let a2 = json!({"git_branch": "feat-git-json"});
    assert_eq!(
        derive_branch(&a2, "").await,
        Some("feat-git-json".to_string())
    );
}

#[tokio::test]
async fn test_derive_branch_from_filesystem() {
    let a = json!({});
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let branch = derive_branch(&a, manifest_dir).await;
    assert!(branch.is_some(), "should derive branch for this repo");
}

#[tokio::test]
async fn test_derive_branch_non_existent_dir() {
    let a = json!({});
    assert_eq!(derive_branch(&a, "/non/existent/path/xyz").await, None);
}

#[tokio::test]
async fn test_derive_branch_rejects_absolute_gitdir() {
    // Pane-controlled `.git` file pointing at an absolute gitdir must
    // not read outside the cwd (join would discard the cwd).
    let dir = std::env::temp_dir().join(format!(
        "ht-gitdir-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".git"), "gitdir: /tmp/elsewhere\n").unwrap();
    let a = json!({});
    assert_eq!(derive_branch(&a, dir.to_str().unwrap()).await, None);
    let _ = std::fs::remove_dir_all(&dir);
}

fn git_present() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The bug: inherited stdout left `wait_with_output` reading an empty
/// buffer (fallback never returned a name) and inherited stderr wrote
/// `fatal: not a git repository` into bot.log per non-git pane.
#[tokio::test]
async fn test_git_branch_cmd_pipes_stdout_and_nulls_stderr() {
    if !git_present() {
        return;
    }
    let mut child = git_branch_cmd("/nonexistent/herdr-test").spawn().unwrap();
    assert!(
        child.stdout.is_some(),
        "stdout must be piped or the branch is never captured"
    );
    assert!(
        child.stderr.is_none(),
        "stderr must be null or git leaks into bot.log"
    );
    let _ = child.wait().await;
}

/// fs fast path bypassed (absolute gitdir rejected) → only the git
/// subprocess can answer, so this fails unless stdout is piped.
#[tokio::test]
async fn test_derive_branch_subprocess_fallback() {
    if !git_present() {
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("ht-fallback-{nanos}"));
    let repo = base.join("repo");
    let pane = base.join("pane");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&pane).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap()])
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    assert!(run(&["init", "-q"]), "git init");
    assert!(
        run(&["symbolic-ref", "HEAD", "refs/heads/feat-fallback"]),
        "git symbolic-ref"
    );
    std::fs::write(
        pane.join(".git"),
        format!("gitdir: {}\n", repo.join(".git").display()),
    )
    .unwrap();

    let branch = derive_branch(&json!({}), pane.to_str().unwrap()).await;
    assert_eq!(branch, Some("feat-fallback".to_string()));
    let _ = std::fs::remove_dir_all(&base);
}
