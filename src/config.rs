use std::env;
use crate::types::Res;

#[derive(Clone, Debug)]
pub struct Cfg {
    pub token: String,
    pub socket: String,
    pub owners: Vec<i64>,
    pub forum: Option<i64>,
}

pub fn load_env_file() {
    let Ok(txt) = std::fs::read_to_string(".env") else { return };
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim();
        let mut v = v.trim().to_string();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = v[1..v.len() - 1].to_string();
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
    let mut owners = Vec::new();
    for (k, v) in env::vars() {
        if (k == "TELEGRAM_CHAT_ID" || k.starts_with("TELEGRAM_CHAT_ID_"))
            && let Ok(id) = v.trim().parse::<i64>()
        {
            owners.push(id);
        }
    }
    owners.sort_unstable();
    owners.dedup();
    if owners.is_empty() {
        return Err("no TELEGRAM_CHAT_ID* owner ids found".into());
    }

    let home = env::var("HOME").unwrap_or_default();
    let socket = env::var("HERDR_SOCKET")
        .unwrap_or_else(|_| format!("{home}/.config/herdr/herdr.sock"));

    let forum = env::var("TELEGRAM_FORUM_CHAT_ID")
        .or_else(|_| env::var("TELEGRAM_GROUP_CHAT_ID"))
        .ok()
        .and_then(|v| {
            let v = v.trim();
            let n: i64 = v.parse().ok()?;
            Some(if v.starts_with('-') {
                n
            } else {
                -(1_000_000_000_000i64 + n)
            })
        });

    Ok(Cfg {
        token,
        socket,
        owners,
        forum,
    })
}
