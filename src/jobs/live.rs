//! Fresh-output accumulation for final-card arbitration (see
//! `report`/`finalize`). The only buzzing message the user gets is the
//! final — activity shows via the typing indicator plus the silent
//! instant message (`progress`: placeholder on submit, transient tail
//! edited in place, deleted when the final lands as NEW).
use crate::{
    jobs::{job::Job, stream::delta},
    types::anchorable_screen,
};
use std::sync::Arc;

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;

/// Track whatever is new into the accumulator (silent; feeds the
/// final-card arbitration — finals are byte-identical to before, the
/// transient tail renders onto the silent instant message in `progress`).
/// Returns true when fresh bytes landed: the runner re-stamps the settle
/// arm on it, so a report commits shortly after the agent actually
/// stopped writing rather than after a fixed wait.
pub async fn stream_live(job: &Arc<Job>, screen: Vec<String>, acc: &mut Vec<String>) -> bool {
    // Outage/empty AND cleared-pane (non-empty all-blank) reads are
    // unknown, never fresh: `watch_stall` returns the blank screen for
    // `note_empty`, and writing it here would poison a good baseline —
    // the scrolled-off fallback in `delta` then reads the whole next
    // scrollback as fresh (dupe/flooded finals). Single source:
    // `anchorable_screen` (Job::new / anchor_baseline parity).
    if !anchorable_screen(&screen) {
        return false;
    }
    if !job.baseline_ok() {
        job.anchor_baseline(screen).await;
        return false;
    }
    let base = job.baseline.lock().await.clone();
    let fresh = delta(&screen, &base);
    if fresh.is_empty() {
        return false;
    }
    // Raw accumulation: boundaries (tool echoes, headers, prompt echo)
    // are resolved at display time so only the fresh reply is shown.
    acc.extend(fresh.iter().cloned());
    if acc.len() > ACC_CAP {
        let drop = acc.len() - ACC_CAP;
        acc.drain(..drop);
    }
    *job.baseline.lock().await = screen;
    true
}

/// Pure: is this epoch bump the publish that CREATED this watcher's job,
/// or a supersede that must drop the previous turn's stream state?
///
/// `enqueue` spawns the watcher BEFORE the submit that creates the job
/// publishes, so the first bump a fresh watcher sees is its own turn.
/// Treating it as a supersede cleared the new turn's first output burst
/// mid-stream, and since the baseline had already advanced nothing refilled
/// it — the transient sat on the placeholder and the final lost its acc
/// body. A job that already carried an epoch when the watcher started
/// (slot reuse, or recovery after a restart) has no such first publish, so
/// every bump it observes is a genuine supersede.
///
/// The `epoch == started_at + 1` clause is load-bearing: two publishes
/// coalescing before the watcher's first iteration (fast succession —
/// exactly the reported leak) observe a single 0→2 bump. Without it that
/// reads as "the creating publish" and skips BOTH the acc clear and the
/// baseline reset, so the next poll re-delivers the previous turn's text
/// as fresh — into the transient AND (when long enough to win
/// arbitration) the final.
pub(crate) fn is_creating_publish(started_at: u64, bumps_seen: u32, epoch: u64) -> bool {
    bumps_seen == 0 && started_at == 0 && epoch == started_at + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_a_fresh_job_s_first_bump_is_its_own_publish() {
        // Fresh job (epoch 0 at spawn): the first bump created this turn —
        // the acc must survive.
        assert!(is_creating_publish(0, 0, 1));
        // Any later bump is a real supersede.
        assert!(!is_creating_publish(0, 1, 2));
        assert!(!is_creating_publish(0, 7, 8));
        // A reused or recovered job already carried an epoch when the
        // watcher started, so it has no creating publish to absorb.
        assert!(!is_creating_publish(1, 0, 2));
        assert!(!is_creating_publish(4, 0, 5));
        // Two publishes coalescing before the first iteration (fast
        // succession) observe one 0→2 bump: NOT the creating publish —
        // skipping the clear + baseline reset here is the reported
        // cross-turn content leak.
        assert!(!is_creating_publish(0, 0, 2));
        assert!(!is_creating_publish(0, 0, 3));
    }

    #[tokio::test]
    async fn test_stream_live_ignores_blank_screen_never_poisons_baseline() {
        // A cleared pane reads as non-empty all-blank lines after
        // watch_stall's note_empty: streaming it would overwrite a good
        // baseline with blanks, and the next real screen would flood the
        // whole scrollback into acc as fresh (Job::anchor_baseline
        // parity — blank reads are outage/unknown, never a delta).
        let job = Job::new(vec!["baseline".into()], 1, None);
        let mut acc = Vec::new();
        stream_live(&job, Vec::new(), &mut acc).await;
        stream_live(&job, vec!["".into(), "   ".into()], &mut acc).await;
        assert!(acc.is_empty());
        assert_eq!(*job.baseline.lock().await, vec!["baseline".to_string()]);
        // Real output still streams and advances the baseline.
        stream_live(&job, vec!["baseline".into(), "fresh".into()], &mut acc).await;
        assert_eq!(acc, vec!["fresh".to_string()]);
        assert_eq!(
            *job.baseline.lock().await,
            vec!["baseline".to_string(), "fresh".to_string()]
        );
    }

    #[tokio::test]
    async fn test_stream_live_unanchored_ignores_blank_anchors_real() {
        // Busy/cleared start: blanks never anchor; the first real screen
        // becomes the baseline without dumping itself as fresh.
        let job = Job::new(Vec::new(), 1, None);
        let mut acc = Vec::new();
        stream_live(&job, vec!["".into(), "  ".into()], &mut acc).await;
        assert!(!job.baseline_ok());
        assert!(acc.is_empty());
        stream_live(&job, vec!["hello".into()], &mut acc).await;
        assert!(job.baseline_ok());
        assert!(acc.is_empty());
        stream_live(&job, vec!["hello".into(), "world".into()], &mut acc).await;
        assert_eq!(acc, vec!["world".to_string()]);
    }
}
