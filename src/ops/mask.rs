//! Display-boundary secret masking (dev.sh `mask()` parity, minus the
//! shell): home dir, bot tokens, chat IDs, `TELEGRAM_*` values. Pure +
//! tested. Files are never rewritten — callers mask only what they print.

/// Mask one display line. Order matters (home before generic user dirs,
/// IDs before token scan). `home` is `$HOME` (empty = skip home steps).
pub fn mask_line(s: &str, home: &str) -> String {
    let mut out = collapse_home_line(s, home);
    out = mask_user_dirs(&out);
    out = mask_chat_id_assign(&out);
    out = mask_forum_ids(&out);
    out = mask_tokens(&out);
    out = mask_telegram_assign(&out);
    out
}

/// `$HOME` anywhere in the line, boundary-checked (the rest must start
/// with `/` or end the line — avoids the `/Users/x2` false positive).
/// Single source: [`crate::types::mask_home_with`] (dup'd literals re-drift).
fn collapse_home_line(s: &str, home: &str) -> String {
    crate::types::mask_home_with(s, home)
}

/// `/Users/<seg>` + `/home/<seg>` → `/Users/***` (other users' names).
fn mask_user_dirs(s: &str) -> String {
    let mut out = s.to_string();
    for prefix in ["/Users/", "/home/"] {
        let mut next = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(prefix) {
            next.push_str(&rest[..i + prefix.len()]);
            let seg: String = rest[i + prefix.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
                .collect();
            if seg.is_empty() {
                rest = &rest[i + prefix.len()..];
            } else {
                next.push_str("***");
                rest = &rest[i + prefix.len() + seg.len()..];
            }
        }
        next.push_str(rest);
        out = next;
    }
    out
}

/// `chat_id = -123` / `chat_id: -123` → `chat_id=***` (YAML/log form
/// uses `:`; only `=` was masked, so `chat_id: -42` leaked the id).
fn mask_chat_id_assign(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("chat_id") {
        let mut j = i + "chat_id".len();
        let b = rest.as_bytes();
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        let mut e = usize::MAX;
        if j < b.len() && (b[j] == b'=' || b[j] == b':') {
            j += 1;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let mut k = j + usize::from(j < b.len() && b[j] == b'-');
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > j + usize::from(j < b.len() && b[j] == b'-') {
                e = k;
            }
        }
        if e != usize::MAX {
            out.push_str(&rest[..i]);
            out.push_str("chat_id=***");
            rest = &rest[e..];
        } else {
            out.push_str(&rest[..i + "chat_id".len()]);
            rest = &rest[i + "chat_id".len()..];
        }
    }
    out.push_str(rest);
    out
}

/// `-100123…` group/forum IDs → `-100***`.
fn mask_forum_ids(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("-100") {
        let mut e = i + 4;
        let b = rest.as_bytes();
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
        if e > i + 4 {
            out.push_str(&rest[..i]);
            out.push_str("-100***");
            rest = &rest[e..];
        } else {
            out.push_str(&rest[..e]);
            rest = &rest[e..];
        }
    }
    out.push_str(rest);
    out
}

/// Bot tokens (`123456:ABC…`, 6+ digits then 20+ tail chars). All
/// inspected bytes are ASCII (digits, `:`, tail class), so byte indices
/// always land on char boundaries.
fn mask_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let (mut i, mut last) = (0, 0);
    while i < s.len() {
        if bytes[i] == b':' {
            let mut d = i;
            while d > 0 && bytes[d - 1].is_ascii_digit() {
                d -= 1;
            }
            let mut e = i + 1;
            while e < s.len()
                && (bytes[e].is_ascii_alphanumeric() || bytes[e] == b'_' || bytes[e] == b'-')
            {
                e += 1;
            }
            if i - d >= 6 && e - (i + 1) >= 20 {
                out.push_str(&s[last..d]);
                out.push_str("***TOKEN***");
                i = e;
                last = e;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

/// `TELEGRAM_FOO=secret` → `TELEGRAM_FOO=***` (value to whitespace).
fn mask_telegram_assign(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("TELEGRAM_") {
        let mut j = i + "TELEGRAM_".len();
        let b = rest.as_bytes();
        while j < b.len() && (b[j].is_ascii_uppercase() || b[j] == b'_') {
            j += 1;
        }
        if j < b.len() && b[j] == b'=' {
            let mut e = j + 1;
            while e < b.len() && !b[e].is_ascii_whitespace() {
                e += 1;
            }
            out.push_str(&rest[..j + 1]);
            out.push_str("***");
            rest = &rest[e..];
        } else {
            out.push_str(&rest[..j]);
            rest = &rest[j..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_line_ordering() {
        let home = "/Users/x";
        // Home collapses (boundary-checked); other users starred.
        assert_eq!(
            mask_line("at /Users/x/a and /Users/y/b", home),
            "at ~/a and /Users/***/b"
        );
        // Mid-name prefix is NOT home (falls to generic user masking).
        assert_eq!(mask_line("at /Users/x2/a", home), "at /Users/***/a");
        // Token gone, IDs gone, env value gone.
        let t = "tok 123456:ABCDEFGHIJKLMNOPQRSTUV chat_id = -42 -100123 TELEGRAM_BOT_TOKEN=abc";
        assert_eq!(
            mask_line(t, home),
            "tok ***TOKEN*** chat_id=*** -100*** TELEGRAM_BOT_TOKEN=***"
        );
        // Clean lines pass through.
        assert_eq!(
            mask_line("bot started (PID: 1)", home),
            "bot started (PID: 1)"
        );
        // Short values are not tokens; bare prefixes untouched.
        assert_eq!(
            mask_line("a 12:34 b -100 c TELEGRAM_X", home),
            "a 12:34 b -100 c TELEGRAM_X"
        );
    }

    #[test]
    fn test_mask_chat_id_colon_form() {
        // YAML/log `chat_id: -42` leaks the numeric id unless `:` is
        // accepted like `=` (mask_chat_id_assign).
        assert_eq!(mask_line("chat_id: -42", ""), "chat_id=***");
        assert_eq!(mask_line("chat_id:-42", ""), "chat_id=***");
        // Non-numeric after `:` still not an id assign (unchanged).
        assert_eq!(mask_line("chat_id: abc", ""), "chat_id: abc");
    }
}
