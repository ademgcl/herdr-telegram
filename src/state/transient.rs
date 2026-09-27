//! `/transient` auto-remove flag: whether the silent working message
//! is deleted when the final lands (on) or kept as readable history
//! (off, the default). Single bool on State (never a map — nothing to
//! prune), persisted to `transient.state` (`on`/`off`, fail-open off).
//! Split from `state` (300-line file limit).
use super::State;
use std::sync::atomic::Ordering;

impl State {
    pub(crate) fn transient_file() -> std::path::PathBuf {
        super::persist_paths::state_dir().join("transient.state")
    }

    /// Fail-open load (tested): only a literal `on` enables — empty,
    /// torn, or hand-edited garbage keeps transients OFF, so a bad write
    /// can never flood the chat with working messages.
    pub(crate) fn load_transient_on() -> bool {
        std::fs::read_to_string(Self::transient_file())
            .map(|s| s.trim().eq_ignore_ascii_case("on"))
            .unwrap_or(false)
    }

    pub fn transient_on(&self) -> bool {
        self.transient_on.load(Ordering::Relaxed)
    }

    /// Flip the flag + persist (short local write, no RPC). Memory
    /// first: a failed disk write keeps the live value for this run
    /// while the next boot re-reads the last good file.
    pub async fn set_transient_on(&self, on: bool) {
        self.transient_on.store(on, Ordering::Relaxed);
        let file = Self::transient_file();
        let tmp = crate::types::unique_tmp(&file);
        if crate::types::write_private(&tmp, if on { b"on" } else { b"off" }).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
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
            std::fs::write(State::transient_file(), content).expect("write");
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
