//! Boot config from env with `.env` fallback (no `set_var`: UB on tokio
//! multi-thread). Env wins, file fills gaps. Pure parser + tests; file
//! reads are boot-only (tiny, race-free — one snapshot per process).
use crate::types::Res;
use std::{collections::HashMap, env};

#[derive(Clone)]
pub struct Cfg {
    pub token: String,
    pub socket: String,
    pub owners: Vec<i64>,
    pub forum: Option<i64>,
}

fn parse_dotenv_text(txt: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // `export KEY=val` form (sourced-shell style): the key is bare.
        // Only when `export` is a standalone prefix word — `exported=1`
        // keeps its name, extra spaces/tabs still strip.
        let kt = k.trim();
        let k = match kt.strip_prefix("export") {
            Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim(),
            _ => kt,
        };
        if k.is_empty() {
            continue;
        }
        let mut v = v.trim().to_string();
        // Quoted values keep everything inside (`Q="a # b"` → `a # b`).
        // Unquoted trailing comments strip (`TOKEN=x # note` → `x`).
        // Quoted + comment (`S='v' # note`): strip the comment first,
        // then one layer of matching quotes — else quotes leak into
        // secrets and boot fails loudly on the wrong value.
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = v[1..v.len() - 1].to_string();
        } else {
            // Unquoted trailing comment (`TOKEN=x # note`): strip the
            // comment only when ` #` sits OUTSIDE quotes, so a quoted
            // value containing ` #` (`Q="a # b" # note`) keeps its inner
            // text instead of corrupting to `"a`.
            if let Some(head) = strip_unquoted_comment(&v) {
                v = head.trim_end().to_string();
            }
            if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                v = v[1..v.len() - 1].to_string();
            }
        }
        // First wins (old set_var-if-absent parity for dup keys).
        map.entry(k.to_string()).or_insert(v);
    }
    map
}

/// Cut a trailing ` # comment` only when it sits outside single/double
/// quotes. Returns None when there is no such comment. Any whitespace
/// (space or tab) may precede the `#`; a backslash-escaped quote inside
/// double quotes never toggles the quote state (single quotes are
/// literal, dotenv-style).
fn strip_unquoted_comment(v: &str) -> Option<String> {
    let b = v.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if q == b'"' && c == b'\\' && i + 1 < b.len() {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else if c == b'"' || c == b'\'' {
            quote = Some(c);
        } else if c.is_ascii_whitespace() && i + 1 < b.len() && b[i + 1] == b'#' {
            return Some(v[..i].to_string());
        }
        i += 1;
    }
    None
}

fn is_owner_key(k: &str) -> bool {
    k == "TELEGRAM_CHAT_ID" || k.starts_with("TELEGRAM_CHAT_ID_")
}

fn dotenv_map() -> HashMap<String, String> {
    // Boot-snapshot cache: one `.env` read per process, so `guard_port`
    // / `herdr_socket` / `cfg_from_env` never skew on a mid-boot edit.
    // Per-process (CWD is fixed at boot), boot/CLI-only, tiny file.
    static CACHE: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            std::fs::read_to_string(".env")
                .map(|t| parse_dotenv_text(&t))
                .unwrap_or_default()
        })
        .clone()
}

/// Env wins, `.env` file fills gaps. Explicitly-set-but-unreadable
/// (non-Unicode) counts as set-but-empty: callers reject loudly via
/// their empty-checks instead of silently taking the file value.
/// (A concurrent unset between the `var_os` presence check and the
/// `var` read also yields `Some("")` — same loud-reject path, safe.)
/// Boot/CLI-only over the boot-snapshot cache (one `.env` read).
pub fn env_or_file(key: &str) -> Option<String> {
    env_or_file_with(&dotenv_map(), key)
}

/// Snapshot variant: `cfg_from_env` parses once and reuses the map for
/// every key (no per-key re-read/TOCTOU skew inside one boot).
fn env_or_file_with(file: &HashMap<String, String>, key: &str) -> Option<String> {
    if env::var_os(key).is_some() {
        return Some(env::var(key).unwrap_or_default());
    }
    file.get(key).cloned()
}

pub fn cfg_from_env() -> Res<Cfg> {
    let file = dotenv_map();
    // Single source: `env_or_file_with` (env wins, file fills gaps).
    let token =
        env_or_file_with(&file, "TELEGRAM_BOT_TOKEN").ok_or("TELEGRAM_BOT_TOKEN not set")?;
    if token.trim().is_empty() {
        return Err("TELEGRAM_BOT_TOKEN must not be empty".into());
    }
    let mut owners = Vec::new();
    // Union env + file keys: `.env`-only owners work without set_var.
    let mut keys: Vec<String> = env::vars()
        .map(|(k, _)| k)
        .filter(|k| is_owner_key(k))
        .collect();
    for k in file.keys() {
        if is_owner_key(k) && !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    for k in keys {
        let v = env_or_file_with(&file, &k).unwrap_or_default();
        let v = v.trim();
        match v.parse::<i64>() {
            Ok(id) if id > 0 => owners.push(id),
            // A group id here boots fine and answers nobody (sender
            // ids are always positive) — fail loudly instead.
            Ok(_) => return Err(format!("{k} must be a positive Telegram user id").into()),
            Err(_) => {
                // Key only: values stay out of the log even when malformed.
                eprintln!("[config] ignoring {k}: not a numeric id");
            }
        }
    }
    owners.sort_unstable();
    owners.dedup();
    if owners.is_empty() {
        return Err("no TELEGRAM_CHAT_ID* owner ids found".into());
    }

    let home = crate::types::home_dir();
    let raw_socket = env_or_file_with(&file, "HERDR_SOCKET").unwrap_or_default();
    // Provenance-aware empty check: an explicitly-set-but-empty value
    // (env OR file) errors fail-closed; only a fully-absent key falls
    // back to the default socket path.
    let socket_set = env::var_os("HERDR_SOCKET").is_some() || file.contains_key("HERDR_SOCKET");
    if raw_socket.trim().is_empty() && socket_set {
        return Err("HERDR_SOCKET must not be empty".into());
    }
    if raw_socket.trim() == "~" {
        return Err("HERDR_SOCKET must be a socket path, not ~".into());
    }
    let socket = {
        let v = raw_socket.trim();
        if v.is_empty() {
            format!("{home}/.config/herdr/herdr.sock")
        } else if let Some(rest) = v.strip_prefix("~/") {
            format!("{home}/{rest}")
        } else {
            v.to_string()
        }
    };

    let forum = env_or_file_with(&file, "TELEGRAM_FORUM_CHAT_ID")
        .or_else(|| env_or_file_with(&file, "TELEGRAM_GROUP_CHAT_ID"))
        .and_then(|v| {
            let v = v.trim();
            match v.parse::<i64>() {
                // Supergroup/forum ids are negative; a positive id is
                // malformed (mirrors owners above) — DM-only with a loud
                // warning instead of minting a bogus negative id that
                // fails every topic call at runtime.
                Ok(n) if v.starts_with('-') && n != 0 => Some(n),
                Ok(_) => {
                    eprintln!("[config] ignoring forum id: must be negative — DM-only mode");
                    None
                }
                Err(_) => {
                    eprintln!("[config] ignoring forum id: not numeric — DM-only mode");
                    None
                }
            }
        });

    Ok(Cfg {
        token,
        socket,
        owners,
        forum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dotenv_quotes_comments_export() {
        let txt = "# c\nTOKEN=x # note\nQ=\"a # b\"\nS='v'\nexport E=1\nexport  F = 2\nexported=3\nEMPTY=\nDUP=1\nDUP=2\n";
        let m = parse_dotenv_text(txt);
        assert_eq!(m.get("TOKEN").map(String::as_str), Some("x"));
        assert_eq!(m.get("Q").map(String::as_str), Some("a # b"));
        assert_eq!(m.get("S").map(String::as_str), Some("v"));
        assert_eq!(m.get("E").map(String::as_str), Some("1"));
        assert_eq!(m.get("F").map(String::as_str), Some("2"));
        assert_eq!(m.get("exported").map(String::as_str), Some("3"));
        // Quoted + trailing comment: comment strips, then quotes.
        let m2 = parse_dotenv_text("S='v' # note\nD=\"a\" # note\n");
        assert_eq!(m2.get("S").map(String::as_str), Some("v"));
        assert_eq!(m2.get("D").map(String::as_str), Some("a"));
        // Quoted value containing ` #` plus a trailing comment: the inner
        // ` #` survives, only the trailing comment strips.
        let m3 = parse_dotenv_text("Q=\"a # b\" # note\nQ2='x # y' # note\n");
        assert_eq!(m3.get("Q").map(String::as_str), Some("a # b"));
        assert_eq!(m3.get("Q2").map(String::as_str), Some("x # y"));
        // Tab before `#` is a comment too; an escaped quote inside
        // double quotes never ends the string early.
        let m4 = parse_dotenv_text("T=v\t# note\nE=\"a \\\" # b\" # note\n");
        assert_eq!(m4.get("T").map(String::as_str), Some("v"));
        assert_eq!(m4.get("E").map(String::as_str), Some("a \\\" # b"));
        assert_eq!(m.get("EMPTY").map(String::as_str), Some(""));
        // First wins (set_var-if-absent parity).
        assert_eq!(m.get("DUP").map(String::as_str), Some("1"));
    }

    #[test]
    fn test_env_or_file_absent_key_is_none() {
        // No env mutation (serial-mutex safe): a certainly-absent key
        // resolves to None rather than a file value.
        assert_eq!(env_or_file("HERDR_TG_TEST_ABSENT_KEY_12345"), None);
    }
}
