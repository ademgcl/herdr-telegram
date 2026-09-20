//! Post-submit re-arm verdict (pure, tested): split from `enqueue`
//! (300-line file limit).

/// The map slot the submit raced against decides. A mapped STOPPED self
/// (runner folds the card before its exit-removal wins the race) still
/// needs the re-cover path — returning bare strands the delivered books
/// watcherless. A mapped live self needs nothing (books landed home);
/// a stopped other (or a vacant slot) needs re-arm; a live other
/// is handled by transfer.
pub(crate) fn rearm_verdict(mapped: Option<(bool, bool)>, self_stopped: bool) -> bool {
    match mapped {
        None => true,
        Some((is_self, mapped_stopped)) if is_self => self_stopped,
        Some((_, mapped_stopped)) => mapped_stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rearm_verdict_matrix() {
        // Vacant slot needs re-arm.
        assert!(rearm_verdict(None, false));
        assert!(rearm_verdict(None, true));
        // Mapped live self needs nothing; stopped self needs re-cover.
        assert!(!rearm_verdict(Some((true, false)), false));
        assert!(rearm_verdict(Some((true, true)), true));
        // Live other handled by transfer (no re-arm); stopped other needs it.
        assert!(!rearm_verdict(Some((false, false)), false));
        assert!(!rearm_verdict(Some((false, false)), true));
        assert!(rearm_verdict(Some((false, true)), false));
        assert!(rearm_verdict(Some((false, true)), true));
    }
}
