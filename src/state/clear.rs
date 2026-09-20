//! Per-pane retire (split from `state/mod`: 300-line file limit).
use super::{
    State,
    guard::{BLOCKOP_STALE_SECS, MODELOP_STALE_SECS, claim_stale},
    persist_paths,
};
use std::sync::Arc;

impl State {
    /// Drop armed input waiters for a dead pane: a typewait surviving
    /// /kill would eat the owner's next message as typed input into a
    /// pane that no longer exists.
    pub async fn clear_pane(self: &Arc<Self>, pane: &str) {
        // Ownership-checked: a concurrent re-mint (same pane name reused)
        // keeps its typing task; a dead pane has no owner so it stops.
        self.stop_typing_unless_owned(pane).await;
        self.clear_waiters(pane).await;
        // Reply targets are NEVER pruned here: a DM reply to a corpse
        // card must fail visibly ("pane gone") via the shell fallback —
        // pruning it would silently reroute the text as a prompt into
        // the focused live agent. Dead entries age out via the 512-cap
        // overflow in `remember`; per-tap `forget_target` drops them.
        // A killed pane must not stay focused: the next bare message
        // would route into the void instead of resolving fresh. Single
        // lock check-and-clear: a set_focus landing between a split check
        // and clear would else wipe the FRESH focus (next bare message
        // loses its target — sole-agent fallback or error).
        {
            let mut focus = self.focus.lock().await;
            if focus.as_deref() == Some(pane) {
                *focus = None;
                let _ = std::fs::remove_file(persist_paths::focus_file());
            }
        }
        self.status.lock().await.remove(pane);
        self.last_done.lock().await.remove(pane);
        self.seen.lock().await.remove(pane);
        self.last_change.lock().await.remove(pane);
        self.debounce.lock().await.remove(pane);
        self.clear_limit_episode(pane).await;
        self.blocked_sig.lock().await.remove(pane);
        self.blocked_card.lock().await.remove(pane);
        self.history.lock().await.remove(pane);
        // Guards die stale-only (never blind): the pane may be a
        // wrong-death verdict (one partial list read) with a tap still in
        // flight — a blind remove lets a second tap claim the freed slot
        // and interleave two key sequences into one dialog. A fresh guard
        // survives (its holder's generation-checked drop releases it on
        // completion; a remint's claim self-evicts it once stale), a
        // corpse evicts now. Remints never inherit either way.
        {
            let now = std::time::Instant::now();
            let mut m = self.modelop.lock().await;
            if m.get(pane)
                .map(|at| claim_stale(*at, now, MODELOP_STALE_SECS))
                .unwrap_or(true)
            {
                m.remove(pane);
            }
        }
        {
            let now = std::time::Instant::now();
            let mut m = self.blockop.lock().await;
            if m.get(pane)
                .map(|at| claim_stale(*at, now, BLOCKOP_STALE_SECS))
                .unwrap_or(true)
            {
                m.remove(pane);
            }
        }
        self.shell_gen.lock().await.remove(pane);
    }
}
