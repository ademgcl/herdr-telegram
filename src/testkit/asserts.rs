//! Chat-scoped assertion helpers for the harness (split from `mod`,
//! 300-line file limit). Every read is filtered to this case's chat, so
//! parallel e2e cases never see each other's traffic in the shared fake.
use super::Harness;
use super::tg_fake;

impl Harness {
    // ---- chat-scoped assertions (this case's traffic only) ----
    pub fn calls(&self, method: &str) -> Vec<tg_fake::Call> {
        self.tg.calls_of(self.chat, method)
    }
    pub fn sends(&self) -> Vec<tg_fake::Call> {
        self.calls("sendMessage")
    }
    pub fn edits(&self) -> Vec<tg_fake::Call> {
        self.calls("editMessageText")
    }
    pub fn sent_texts(&self) -> Vec<String> {
        self.tg.sent_texts(self.chat)
    }
    pub fn sent_count(&self, method: &str) -> usize {
        self.tg.sent_count(self.chat, method)
    }
    pub fn fault(&self, method: &str, description: &str) {
        self.tg.fault_next(self.chat, method, description);
    }
    /// Fault for a method that carries no chat id (getFile), scoped to
    /// this case's file id — an unscoped bucket would fail a parallel
    /// case's happy path.
    pub fn fault_any(&self, method: &str, file_id: &str, description: &str) {
        self.tg.fault_global(method, file_id, description);
    }
}
