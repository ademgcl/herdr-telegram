//! Per-boot control-socket token: minted by the server into
//! `<state_dir>/ctl.token` (0600), read back by the CLI client. Split
//! from `ctl` (300-line file limit).
use std::os::unix::fs::OpenOptionsExt;

/// Path of the per-boot control token (0600, same user only).
pub fn ctl_token_path() -> std::path::PathBuf {
    crate::state::state_dir().join("ctl.token")
}

pub fn read_control_token() -> Option<String> {
    std::fs::read_to_string(ctl_token_path())
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Mint a 48-hex-char token from the OS RNG. Retries a dead urandom a
/// few times, then gives up (None): the server runs WITHOUT its control
/// socket rather than with a guessable token (fail-closed — clients get
/// a clear "token missing" error). Unreachable on macOS/Linux, where
/// urandom cannot fail while TLS (required anyway) still works.
pub fn new_control_token() -> Option<String> {
    use std::io::Read;
    for _ in 0..5 {
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let mut buf = [0u8; 24];
            if f.read_exact(&mut buf).is_ok() && buf.iter().any(|&b| b != 0) {
                return Some(buf.iter().map(|b| format!("{b:02x}")).collect());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// Pure auth-line check (unit-tested): exactly `auth <token>`.
pub fn auth_line_ok(line: &str, token: &str) -> bool {
    line.trim() == format!("auth {token}")
}

/// Render the token path for user-facing errors without the username.
pub fn display_token_path() -> String {
    crate::types::collapse_home(
        &ctl_token_path().display().to_string(),
        &std::env::var("HOME").unwrap_or_default(),
    )
}

/// Persist the token 0600 so only this user can read it back.
/// `mode(0o600)` applies at create time only — a pre-existing 0644 file
/// keeps its width — so chmod after open like `write_private` (same rule).
pub fn write_control_token(token: &str) {
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(ctl_token_path())
    {
        Ok(mut f) => {
            use std::os::unix::fs::PermissionsExt;
            if f.set_permissions(std::fs::Permissions::from_mode(0o600)).is_err() {
                eprintln!("[ctl] warning: control token file may be wider than 0600");
            }
            use std::io::Write;
            if writeln!(f, "{token}").is_err() {
                eprintln!("[ctl] warning: cannot persist control token");
            }
        }
        Err(e) => eprintln!("[ctl] warning: cannot persist control token: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_token_shape_and_uniqueness() {
        // 24 random bytes as 48 lowercase hex chars; two mints differ.
        for t in [new_control_token(), new_control_token()] {
            let t = t.expect("urandom available in test");
            assert_eq!(t.len(), 48);
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(new_control_token(), new_control_token());
    }

    #[test]
    fn test_auth_line_exact_match_only() {
        assert!(auth_line_ok("auth abc123", "abc123"));
        assert!(auth_line_ok("auth abc123\n", "abc123")); // line discipline trims
        assert!(!auth_line_ok("auth wrong", "abc123"));
        assert!(!auth_line_ok("", "abc123"));
        assert!(!auth_line_ok("auth", "abc123"));
        assert!(!auth_line_ok("auth abc123 extra", "abc123"));
    }
}
