pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TG_POLL_SECS: i64 = 25;
pub const STALE_SECS: u64 = 600;
pub const LIVE_TICK_SECS: u64 = 4;
pub const LIVE_EDIT_COOLDOWN_SECS: u64 = 4;
pub const MAX_MSG_UNITS: usize = 3900;
pub const SINGLE_INSTANCE_PORT: u16 = 47319;

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
