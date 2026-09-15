use crate::types::Res;
use std::env;

#[derive(Clone)]
pub struct Cfg {
    pub token: String,
    pub socket: String,
    pub owners: Vec<i64>,
    pub forum: Option<i64>,
}

pub fn load_env_file() {
    let Ok(txt) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // `export KEY=val` form (sourced-shell style): the key is bare.
        let k = k.trim().strip_prefix("export ").unwrap_or(k.trim()).trim();
        if k.is_empty() {
            continue;
        }
        let mut v = v.trim().to_string();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = v[1..v.len() - 1].to_string();
        } else if let Some((head, _)) = v.split_once(" #") {
            // Unquoted trailing comment (`TOKEN=x # note`). Quoted values
            // keep everything (secrets may contain `#`).
            v = head.trim_end().to_string();
        }
        if env::var_os(k).is_none() {
            // Safe single-threaded environment seeding during init
            unsafe { env::set_var(k, v) };
        }
    }
}

pub fn cfg_from_env() -> Res<Cfg> {
    load_env_file();
    let token = env::var("TELEGRAM_BOT_TOKEN").map_err(|_| "TELEGRAM_BOT_TOKEN not set")?;
    if token.trim().is_empty() {
        return Err("TELEGRAM_BOT_TOKEN must not be empty".into());
    }
    let mut owners = Vec::new();
    for (k, v) in env::vars() {
        if k == "TELEGRAM_CHAT_ID" || k.starts_with("TELEGRAM_CHAT_ID_") {
            let v = v.trim();
            match v.parse::<i64>() {
                Ok(id) if id > 0 => owners.push(id),
                // A group id here boots fine and answers nobody (sender
                // ids are always positive) — fail loudly instead.
                Ok(_) => return Err(format!("{k} must be a positive Telegram user id").into()),
                Err(_) => {
                    eprintln!("[config] ignoring {k}: not a numeric id ({v:?})");
                }
            }
        }
    }
    owners.sort_unstable();
    owners.dedup();
    if owners.is_empty() {
        return Err("no TELEGRAM_CHAT_ID* owner ids found".into());
    }

    let home = env::var("HOME").unwrap_or_default();
    let raw_socket = env::var("HERDR_SOCKET").unwrap_or_default();
    if raw_socket.trim().is_empty() && env::var_os("HERDR_SOCKET").is_some() {
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

    let forum = env::var("TELEGRAM_FORUM_CHAT_ID")
        .or_else(|_| env::var("TELEGRAM_GROUP_CHAT_ID"))
        .ok()
        .and_then(|v| {
            let v = v.trim();
            match v.parse::<i64>() {
                Ok(n) => Some(if v.starts_with('-') {
                    n
                } else {
                    -(1_000_000_000_000i64 + n)
                }),
                Err(_) => {
                    eprintln!("[config] ignoring forum id: not numeric ({v:?}) — DM-only mode");
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
