//! Fresh-output accumulation for final-card arbitration (see
//! `report`/`finalize`). No live card is ever posted: the only message
//! the user gets is the buzzing final — activity shows via the typing
//! indicator, never a working box. (A live-slot address/throttle once
//! lived here; nothing ever posted through it, so it was removed —
//! zero dead code.)
use crate::{
    jobs::{job::Job, stream::delta},
    types::anchorable_screen,
};
use std::sync::Arc;

/// Cap streamed accumulation: tail-shaped window, never unbounded.
const ACC_CAP: usize = 400;

/// Track whatever is new into the accumulator (silent; feeds the
/// final-card arbitration — finals are byte-identical to before, only
/// the working box is gone).
pub async fn stream_live(job: &Arc<Job>, screen: Vec<String>, acc: &mut Vec<String>) {
    // Outage/empty AND cleared-pane (non-empty all-blank) reads are
    // unknown, never fresh: `watch_stall` returns the blank screen for
    // `note_empty`, and writing it here would poison a good baseline —
    // the scrolled-off fallback in `delta` then reads the whole next
    // scrollback as fresh (dupe/flooded finals). Single source:
    // `anchorable_screen` (Job::new / anchor_baseline parity).
    if !anchorable_screen(&screen) {
        return;
    }
    if !job.baseline_ok() {
        job.anchor_baseline(screen).await;
        return;
    }
    let base = job.baseline.lock().await.clone();
    let fresh = delta(&screen, &base);
    if fresh.is_empty() {
        return;
    }
    // Raw accumulation: boundaries (tool echoes, headers, prompt echo)
    // are resolved at display time so only the fresh reply is shown.
    acc.extend(fresh.iter().cloned());
    if acc.len() > ACC_CAP {
        let drop = acc.len() - ACC_CAP;
        acc.drain(..drop);
    }
    *job.baseline.lock().await = screen;
}

#[cfg(test)]
mod tests {
    use super::*;

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
