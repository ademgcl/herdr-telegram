//! Tests for the migration-only CAS variant (split: 300-line file limit).
use crate::state::cancel::isolated_state;

#[tokio::test]
async fn test_migrate_cas_never_resurrects_vacant() {
    // A vacant slot means cancelled/settled: the remap must not mint the
    // corpse text there with a fresh stamp (immortal resurrected work).
    let (s, _dir) = isolated_state();
    assert!(
        !s.migrate_pending_cas("t:p1", (1, Some(7), "hi"), (1, Some(9), "hi"))
            .await
    );
    assert!(!s.pending.lock().await.contains_key("t:p1"));
}

#[tokio::test]
async fn test_migrate_cas_moves_occupied_match() {
    let (s, _dir) = isolated_state();
    s.remember_pending("t:p1", 1, Some(7), "hi").await;
    let stamp = s.pending.lock().await["t:p1"].started_unix;
    assert!(
        s.migrate_pending_cas("t:p1", (1, Some(7), "hi"), (1, Some(9), "hi"))
            .await
    );
    let p = s.pending.lock().await["t:p1"].clone();
    assert_eq!((p.chat, p.thread, p.prompt.as_str()), (1, Some(9), "hi"));
    assert_eq!(p.started_unix, stamp);
    // Foreign triple never clobbers.
    assert!(
        !s.migrate_pending_cas("t:p1", (1, Some(7), "hi"), (1, Some(11), "hi"))
            .await
    );
    assert_eq!(s.pending.lock().await["t:p1"].thread, Some(9));
}
