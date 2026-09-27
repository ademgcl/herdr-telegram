//! `/transient` auto-remove flag: whether the silent working message
//! is deleted when the final lands (on) or kept as readable history
//! (off, the default). Single bool on State (never a map — nothing to
//! prune), persisted to `transient.state` (`on`/`off`, fail-open off).
//! Split from `state` (500-line file limit).
use super::State;
use std::sync::atomic::Ordering;

impl State {
    fn flag_file(name: &str) -> std::path::PathBuf {
        super::persist_paths::state_dir().join(format!("{name}.state"))
    }

    /// `on`/`off`, with a default for empty/torn/garbage — never a
    /// guess. The two flags default in OPPOSITE directions on purpose:
    /// a bad read of `transient` must not post working messages, and a
    /// bad read of `shape` must not restore unreadable replies.
    fn read_flag(name: &str, default: bool) -> bool {
        match std::fs::read_to_string(Self::flag_file(name)) {
            Ok(s) if s.trim().eq_ignore_ascii_case("on") => true,
            Ok(s) if s.trim().eq_ignore_ascii_case("off") => false,
            _ => default,
        }
    }

    fn write_flag(name: &str, on: bool) {
        let file = Self::flag_file(name);
        let tmp = crate::types::unique_tmp(&file);
        let want: &[u8] = if on { b"on" } else { b"off" };
        if crate::types::write_private(&tmp, want).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }

    /// Fail-open load (tested): only a literal `on` enables — empty,
    /// torn, or hand-edited garbage keeps transients OFF, so a bad write
    /// can never flood the chat with working messages.
    pub(crate) fn load_transient_on() -> bool {
        Self::read_flag("transient", false)
    }

    /// Fail-open to ON: a torn write must not silently restore
    /// unreadable replies.
    pub(crate) fn load_shape_telegram() -> bool {
        Self::read_flag("shape", true)
    }

    pub fn transient_on(&self) -> bool {
        self.transient_on.load(Ordering::Relaxed)
    }

    /// Flip the flag + persist (short local write, no RPC). Memory
    /// first: a failed disk write keeps the live value for this run
    /// while the next boot re-reads the last good file.
    /// Phone-shape final cards for Telegram (see `telegram::shape`).
    /// Separate from `transient_on`: that is "do I want working
    /// messages", this is "make the reply readable on a phone". Default
    /// on — an unreadable reply is the bug, not the feature.
    pub fn shape_telegram(&self) -> bool {
        self.shape_telegram.load(Ordering::Relaxed)
    }

    pub async fn set_shape_telegram(&self, on: bool) {
        self.shape_telegram.store(on, Ordering::Relaxed);
        Self::write_flag("shape", on);
    }

    pub async fn set_transient_on(&self, on: bool) {
        self.transient_on.store(on, Ordering::Relaxed);
        Self::write_flag("transient", on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transient_load_fail_open_off() {
        // Only a literal `on` enables; missing/torn/garbage keeps
        // transients OFF (fail-open).
        let (_s, _dir) = crate::state::cancel::isolated_state();
        assert!(!State::load_transient_on());
        for (content, want) in [
            ("on", true),
            ("ON", true),
            ("  on\n", true),
            ("off", false),
            ("", false),
            ("yes", false),
            ("onn", false),
        ] {
            std::fs::write(State::flag_file("transient"), content).expect("write");
            assert_eq!(State::load_transient_on(), want, "{content:?}");
        }
    }

    #[tokio::test]
    async fn test_transient_set_persists_and_reloads() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        assert!(!s.transient_on());
        s.set_transient_on(true).await;
        assert!(s.transient_on());
        assert!(State::load_transient_on());
        s.set_transient_on(false).await;
        assert!(!s.transient_on());
        assert!(!State::load_transient_on());
    }
}
