//! Shared shell/keys input validation: split from `shell_run`
//! (300-line file limit). Single source for the caps so every entry
//! point refuses the same way (fail-closed: no mint, no RPC, no silent
//! truncation on absurd input).
/// Max shell words per command (one message ≈ 4k chars of paste is
/// ~2000 words into one RPC — refuse, never partially run).
pub const SHELL_CMD_MAX_WORDS: usize = 256;
/// Max keys per send (one message is a handful of keys, never ~2000
/// args into one `send_keys` — refuse, never silently truncate).
pub const KEYS_MAX: usize = 32;
/// Max chars per shell/keys input: the word/key caps count tokens, so
/// a single 4000-char blob (base64, no spaces) would else bypass them
/// into one RPC + Telegram echo. Refuse, never partially run.
pub const INPUT_MAX_CHARS: usize = 4096;

/// Fail-closed shell validation (pure): trims, then refuses empty and
/// over-cap. Returns the trimmed command on success.
pub fn validate_shell_cmd(cmd: &str) -> Result<&str, &'static str> {
    let t = cmd.trim();
    if t.is_empty() {
        return Err("empty command — nothing to run");
    }
    if t.chars().count() > INPUT_MAX_CHARS {
        return Err("command too long — shorten it");
    }
    if t.split_whitespace().count() > SHELL_CMD_MAX_WORDS {
        return Err("command too long — shorten it");
    }
    Ok(t)
}

/// Fail-closed keys validation (pure): refuses empty and over-cap.
/// Never truncates — a partial write is worse than a refusal.
pub fn validate_keys_len(n: usize) -> Result<(), String> {
    if n == 0 {
        return Err("empty keys — nothing sent".to_string());
    }
    if n > KEYS_MAX {
        return Err(format!("too many keys — shorten it (max {KEYS_MAX})"));
    }
    Ok(())
}

/// Fail-closed keys-text validation (pure, single source with the
/// len gate above): refuses empty, over-count, and over-char inputs.
/// Callers with the raw text should use this; callers holding only a
/// count keep `validate_keys_len`.
pub fn validate_keys_text(text: &str) -> Result<Vec<&str>, String> {
    if text.chars().count() > INPUT_MAX_CHARS {
        return Err(format!(
            "too many keys — shorten it (max {INPUT_MAX_CHARS} chars)"
        ));
    }
    let keys: Vec<&str> = text.split_whitespace().collect();
    validate_keys_len(keys.len())?;
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_cmd_empty_and_cap() {
        assert!(validate_shell_cmd("   ").is_err());
        assert_eq!(validate_shell_cmd("  ls -la ").unwrap(), "ls -la");
        let big = vec!["w"; SHELL_CMD_MAX_WORDS + 1].join(" ");
        assert!(validate_shell_cmd(&big).is_err());
        let ok = vec!["w"; SHELL_CMD_MAX_WORDS].join(" ");
        assert!(validate_shell_cmd(&ok).is_ok());
    }

    #[test]
    fn test_keys_len_bounds() {
        assert!(validate_keys_len(0).is_err());
        assert!(validate_keys_len(1).is_ok());
        assert!(validate_keys_len(KEYS_MAX).is_ok());
        assert!(validate_keys_len(KEYS_MAX + 1).is_err());
        // Single huge token bypasses word/key counts — char cap stops it.
        assert!(validate_shell_cmd(&"x".repeat(INPUT_MAX_CHARS + 1)).is_err());
        assert!(validate_keys_text(&"x".repeat(INPUT_MAX_CHARS + 1)).is_err());
        assert!(validate_keys_text("y enter").is_ok());
    }
}
