pub mod cards;
pub mod dm;
pub mod hygiene;
pub mod hygiene_flip;
pub mod limit_decide;
pub mod limits;
pub mod reconcile;
pub mod reconcile_close;
pub mod reconcile_tail;
pub mod spontaneous;
pub mod status;

/// Single source for the post-prompt quiet window (a final card makes a
/// same-pane idle/done alert redundant). Dup'd literals re-drift.
pub(crate) const POST_PROMPT_QUIET_SECS: u64 = 45;

pub use limits::LIMIT_REMIND_SECS;
pub use reconcile::reconcile;
pub use status::observe_status;
