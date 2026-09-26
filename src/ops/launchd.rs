//! launchd prod install: render the service plist + bootstrap it.
//! Replaces dev.herdr.telegram.plist.example + the README sed recipe —
//! every key below is byte-identical in meaning (ThrottleInterval 30 +
//! rationale included). Split from `ops` (300-line file limit).
//!
//! install is idempotent (bootout → write → bootstrap) and never kills:
//! a busy guard port refuses instead of murdering a foreign owner.
//! uninstall is bootout + rm only — it never touches supervised PIDs.
use super::{proc, say};
use crate::types::Res;
use std::path::PathBuf;

pub const LABEL: &str = "dev.herdr.telegram";

pub fn plist_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("Library/LaunchAgents/dev.herdr.telegram.plist")
}

/// Prod must run the release binary (state/log/cwd parity with the
/// documented setup); a debug `dev install` would pin a dev build.
pub fn release_exe(exe: &str) -> bool {
    // Component-exact, never substring: `…/target/release-evil/…` or
    // `…/target/release.bak/debug/…` must not pass the gate.
    let mut parts = exe.split('/').rev();
    let file_ok = parts.next().is_some_and(|f| !f.is_empty());
    file_ok && parts.next() == Some("release") && parts.next() == Some("target")
}

pub fn render_plist(exe: &str, dir: &str) -> String {
    let exe = xml_escape(exe);
    let dir = xml_escape(dir);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{exe}</string>\n\
         \t</array>\n\
         \t<key>WorkingDirectory</key>\n\
         \t<string>{dir}</string>\n\
         \t<!-- ThrottleInterval 30: the bot holds a single-instance TCP guard, so a fast crash-loop would spin hot against a live/stale holder; throttling keeps restarts spaced out. -->\n\
         \t<key>ThrottleInterval</key>\n\
         \t<integer>30</integer>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<!-- SuccessfulExit must nest under KeepAlive: a sibling\n\
         \t\t     key is ignored, so restart-only-on-failure never\n\
         \t\t     applied and a clean exit still relaunched forever. -->\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{dir}/bot.log</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{dir}/bot.log</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// Minimal XML escape for plist string values: a legal `&`/`<`/`>` in
/// the repo path would else yield an invalid plist and a failed
/// bootstrap. Pure so it is unit-tested.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

async fn launchctl(args: &[&str]) -> (bool, String) {
    let out = tokio::process::Command::new("launchctl")
        .args(args)
        .output()
        .await;
    match out {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

async fn uid() -> Res<String> {
    let out = tokio::process::Command::new("id")
        .arg("-u")
        .output()
        .await
        .map_err(|e| format!("id -u: {e}"))?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("id -u returned {id:?}").into());
    }
    Ok(id)
}

pub async fn install(home: &str) -> Res<()> {
    if home.is_empty() {
        return Err(crate::types::HOME_NOT_SET.into());
    }
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let exe = exe.to_string_lossy().into_owned();
    if !release_exe(&exe) {
        return Err("run dev install from ./target/release/herdr-telegram".into());
    }
    let dir = std::env::current_dir().map_err(|e| format!("current dir: {e}"))?;
    let dir = dir.to_string_lossy().into_owned();
    let path = plist_path(home);
    let id = uid().await?;
    // Unload a previous revision first (tolerate not-loaded);
    // bootstrap errors when the job already exists.
    let _ = launchctl(&["bootout", &format!("gui/{id}/{LABEL}")]).await;
    // Fail-closed: a STILL-busy guard means a foreign owner (dev
    // console, manual run) — restore the previous job (old plist is
    // untouched) and refuse instead of orphaning prod. Every early
    // return below restores too: bootout already ran, so a bad port or
    // a failed plist write must not leave prod DOWN.
    let port = match proc::guard_port() {
        Ok(p) => p,
        Err(e) => {
            let (ok, err) =
                launchctl(&["bootstrap", &format!("gui/{id}"), &path.to_string_lossy()]).await;
            if !ok {
                return Err(format!("{e} (previous job restore FAILED: {err})").into());
            }
            return Err(e);
        }
    };
    if proc::port_busy(port) || !proc::bot_pids().is_empty() {
        // The previous job was bootouted above: a failed restore leaves
        // prod DOWN while the error says only "guard busy". Report the
        // restore outcome loudly instead of swallowing it.
        let (ok, err) =
            launchctl(&["bootstrap", &format!("gui/{id}"), &path.to_string_lossy()]).await;
        if !ok {
            return Err(format!(
                "guard busy — dev stop/cleanup first, then dev install (previous job restore FAILED: {err})"
            )
            .into());
        }
        return Err("guard busy — dev stop/cleanup first, then dev install".into());
    }
    // Backup the old plist before overwriting: a failed bootstrap
    // below must restore file + job, never leave prod DOWN.
    let old_plist = std::fs::read(&path).ok();
    if let Err(e) = std::fs::write(&path, render_plist(&exe, &dir)) {
        let (ok, rerr) =
            launchctl(&["bootstrap", &format!("gui/{id}"), &path.to_string_lossy()]).await;
        if !ok {
            return Err(format!("write plist: {e} (previous job restore FAILED: {rerr})").into());
        }
        return Err(format!("write plist: {e}").into());
    }
    let (ok, err) = launchctl(&["bootstrap", &format!("gui/{id}"), &path.to_string_lossy()]).await;
    if !ok {
        if let Some(bytes) = old_plist {
            let _ = std::fs::write(&path, &bytes);
        } else {
            let _ = std::fs::remove_file(&path);
        }
        let (rok, rerr) =
            launchctl(&["bootstrap", &format!("gui/{id}"), &path.to_string_lossy()]).await;
        if !rok {
            return Err(
                format!("bootstrap failed: {err} (previous job restore FAILED: {rerr})").into(),
            );
        }
        return Err(format!("bootstrap failed: {err} (previous plist restored)").into());
    }
    say(
        home,
        &format!("installed {} (logs to bot.log)", path.display()),
    );
    Ok(())
}

pub async fn uninstall(home: &str) -> Res<()> {
    if home.is_empty() {
        return Err(crate::types::HOME_NOT_SET.into());
    }
    let id = uid().await?;
    // Bootout + rm only — never stop_all: a supervised dev child is
    // not ours to kill, and prod restarts itself while bootstrapped.
    let _ = launchctl(&["bootout", &format!("gui/{id}/{LABEL}")]).await;
    match tokio::fs::remove_file(plist_path(home)).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove plist: {e}").into()),
    }
    say(home, "uninstalled dev.herdr.telegram");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_plist_keeps_every_key() {
        let p = render_plist("/r/target/release/herdr-telegram", "/r");
        for key in [
            "<string>dev.herdr.telegram</string>",
            "<string>/r/target/release/herdr-telegram</string>",
            "<string>/r</string>",
            "<!-- ThrottleInterval 30:",
            "<integer>30</integer>",
            "<key>RunAtLoad</key>",
            "<key>KeepAlive</key>",
            "<key>SuccessfulExit</key>",
            "<string>/r/bot.log</string>",
            "<!DOCTYPE plist",
        ] {
            assert!(p.contains(key), "plist missing {key}");
        }
        assert!(!p.contains("__REPO_DIR__"), "placeholder leaked");
        // SuccessfulExit must NEST under KeepAlive (sibling key is
        // ignored by launchd — "restart only on failure" never applied).
        let ka = p.find("<key>KeepAlive</key>").expect("KeepAlive");
        let se = p.find("<key>SuccessfulExit</key>").expect("SuccessfulExit");
        let ka_end = p
            .find("<key>StandardOutPath</key>")
            .expect("after KeepAlive");
        assert!(
            ka < se && se < ka_end,
            "SuccessfulExit not inside KeepAlive"
        );
        assert!(p[ka..ka_end].contains("<dict>"), "KeepAlive must be a dict");
    }

    #[test]
    fn test_release_exe_gate() {
        assert!(release_exe("/r/target/release/herdr-telegram"));
        assert!(!release_exe("/r/target/debug/herdr-telegram"));
        // Substring lookalikes must not pass the prod gate.
        assert!(!release_exe("/tmp/target/release-evil/herdr-telegram"));
        assert!(!release_exe("/r/target/release.bak/debug/herdr-telegram"));
        assert!(!release_exe("herdr-telegram"));
    }

    #[test]
    fn test_plist_path_suffix() {
        let p = plist_path("/Users/x");
        assert!(p.ends_with("Library/LaunchAgents/dev.herdr.telegram.plist"));
    }

    #[test]
    fn test_render_plist_escapes_xml() {
        let p = render_plist("/r/a&b/herdr-telegram", "/r/a<b>");
        assert!(p.contains("/r/a&amp;b/herdr-telegram"));
        assert!(p.contains("/r/a&lt;b&gt;"));
    }
}
