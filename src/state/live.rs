//! Shared transient slot: ONE silent working message per pane, edited
//! across all turns (thinking → working tail → …). A new message is
//! posted only when no slot exists — so after the very first muted
//! entry, every update is an edit (edits never notify) and only finals
//! buzz. `turn` bumps once per turn in `ensure_instant`: finalize
//! deletes only at the entry generation, so a stale turn can never
//! take a successor's message. `live_sending` single-flights posts
//! across tasks (enqueue instant vs watcher ticks). Both maps are
//! pane-keyed: pruned with pane death in hygiene (live-only retain),
//! never persisted (rebuilt on demand). Split from `state` (300-line
//! file limit). Locks never cross RPC — snapshot, drop, then call.
use super::State;

/// One pane's transient pointer: where the working message lives, what
/// it shows, and which turn owns it.
#[derive(Clone)]
pub(crate) struct LiveSlot {
    pub chat: i64,
    pub thread: Option<i64>,
    pub mid: i64,
    pub text: String,
    pub at: Option<std::time::Instant>,
    pub turn: u64,
}

impl State {
    pub(crate) async fn live_get(&self, pane: &str) -> Option<LiveSlot> {
        self.live.lock().await.get(pane).cloned()
    }

    pub(crate) async fn live_put(&self, pane: &str, slot: LiveSlot) {
        self.live.lock().await.insert(pane.to_string(), slot);
    }

    pub(crate) async fn live_take(&self, pane: &str) -> Option<LiveSlot> {
        self.live.lock().await.remove(pane)
    }

    /// Compare-and-take: only when the slot still holds (`mid`, `gen`)
    /// (a successor's claim bump or fresh message survives our retire).
    pub(crate) async fn live_take_if(&self, pane: &str, mid: i64, turn: u64) -> Option<LiveSlot> {
        let mut m = self.live.lock().await;
        if m.get(pane).is_some_and(|s| s.mid == mid && s.turn == turn) {
            m.remove(pane)
        } else {
            None
        }
    }

    /// Claim the post flight for `pane`: false when another task is
    /// already sending (the loser skips; its next tick sees the slot).
    pub(crate) async fn live_claim(&self, pane: &str) -> bool {
        self.live_sending.lock().await.insert(pane.to_string())
    }

    pub(crate) async fn live_unclaim(&self, pane: &str) {
        self.live_sending.lock().await.remove(pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(mid: i64, turn: u64) -> LiveSlot {
        LiveSlot {
            chat: 7,
            thread: None,
            mid,
            text: "t".to_string(),
            at: None,
            turn,
        }
    }

    #[tokio::test]
    async fn test_live_slot_roundtrip() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        assert!(s.live_get("w1:p1").await.is_none());
        s.live_put("w1:p1", slot(11, 1)).await;
        assert_eq!(s.live_get("w1:p1").await.unwrap().mid, 11);
        // take_if refuses a moved-on message, takes the expected one.
        assert!(s.live_take_if("w1:p1", 99, 1).await.is_none());
        assert!(s.live_take_if("w1:p1", 11, 2).await.is_none());
        assert!(s.live_get("w1:p1").await.is_some());
        assert_eq!(s.live_take_if("w1:p1", 11, 1).await.unwrap().turn, 1);
        assert!(s.live_take("w1:p1").await.is_none());
    }

    #[tokio::test]
    async fn test_live_claim_single_flight() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        assert!(s.live_claim("w1:p1").await);
        assert!(!s.live_claim("w1:p1").await);
        s.live_unclaim("w1:p1").await;
        assert!(s.live_claim("w1:p1").await);
        s.live_unclaim("w1:p1").await;
    }
}
