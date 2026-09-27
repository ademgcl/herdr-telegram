//! The phone-shaping pref: default on, and a no-op on a short reply.
//!
//! Split out of `telegram::shape` so that module stays dependency-free
//! and can be shared verbatim by the `tgshape` CLI (see
//! `src/bin/tgshape.rs`) — one implementation, not two.
#[tokio::test]
async fn test_shape_flag_defaults_on_and_persists() {
    let (s, _dir) = crate::state::cancel::isolated_state();
    assert!(s.shape_telegram(), "readable replies are the default");
    s.set_shape_telegram(false).await;
    assert!(!s.shape_telegram(), "off is honoured in memory");
    assert!(!crate::state::State::load_shape_telegram(), "off persists");
    s.set_shape_telegram(true).await;
    assert!(s.shape_telegram());
    assert!(crate::state::State::load_shape_telegram());
}

/// The gate must be a no-op for the common case, so the common case
/// costs nothing and cannot regress.
#[test]
fn test_shaping_a_short_reply_is_a_no_op() {
    let short = "Fixed. The card is the reply now — nothing else changed.";
    assert_eq!(crate::telegram::shape::shape(short), short);
}
