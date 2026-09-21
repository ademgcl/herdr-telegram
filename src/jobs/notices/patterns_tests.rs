//! Match-table parity tests (split from `detect_tests`, 300-line file limit).
use super::super::types::ERROR_KIND;
use super::*;

#[test]
fn test_fatal_markers_never_drift_from_strong() {
    // Single-source drift guard: FATAL_PROVIDER_MARKERS hand-mirrors
    // STRONG literals (the chrome bypass + settle arbitration key on
    // it). A new fatal shape added to STRONG without FATAL gets
    // ┃-framed lines stripped as chrome — blanking the tail so a stale
    // turn resurfaces as the "answer". Both directions pinned: every
    // FATAL marker is a STRONG pattern, and every STRONG error-kind
    // pattern is a FATAL marker.
    for m in FATAL_PROVIDER_MARKERS {
        assert!(
            STRONG.iter().any(|(p, _)| p == m),
            "FATAL marker orphaned from STRONG: {m:?}"
        );
    }
    for (p, kind) in STRONG {
        if *kind == ERROR_KIND {
            assert!(
                FATAL_PROVIDER_MARKERS.contains(p),
                "STRONG error pattern missing from FATAL: {p:?}"
            );
        }
    }
}
