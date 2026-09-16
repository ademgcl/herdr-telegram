pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TG_POLL_SECS: i64 = 25;
pub const STALE_SECS: u64 = 600;
pub const LIVE_EDIT_COOLDOWN_SECS: u64 = 4;
pub const MAX_MSG_UNITS: usize = 3900;
pub const SINGLE_INSTANCE_PORT: u16 = 47319;
/// Herdr socket protocol this bot is built against (see `herdr api schema`).
pub const HERDR_PROTOCOL: u64 = 22;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRow {
    pub kind: String,
    pub pane: String,
    pub title: String,
    pub status: String,
    pub ws: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDetail {
    pub kind: String,
    pub pane: String,
    pub title: String,
    pub status: String,
    pub ws: String,
    pub cwd: String,
    pub branch: Option<String>,
}

impl From<AgentDetail> for AgentRow {
    fn from(d: AgentDetail) -> Self {
        Self {
            kind: d.kind,
            pane: d.pane,
            title: d.title,
            status: d.status,
            ws: d.ws,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub label: String,
    pub number: u64,
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    pub chat_id: i64,
    pub message_thread_id: Option<i64>,
    pub text: String,
}

/// Collapse $HOME to `~` in a display path (log hygiene: no username
/// leak). Pure so it is unit-tested; empty home leaves the path alone.
pub fn collapse_home(path: &str, home: &str) -> String {
    if home.is_empty() {
        path.to_string()
    } else {
        path.replacen(home, "~", 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_home() {
        assert_eq!(collapse_home("/Users/x/.config/a", "/Users/x"), "~/.config/a");
        assert_eq!(collapse_home("/other/path", "/Users/x"), "/other/path");
        assert_eq!(collapse_home("/Users/x/a", ""), "/Users/x/a");
    }
}
