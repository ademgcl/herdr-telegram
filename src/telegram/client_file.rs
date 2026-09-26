//! Telegram file download (split from `client`, 300-line file limit).
use super::TelegramClient;
use crate::types::Res;
use std::time::Duration;

impl TelegramClient {
    /// Download a Telegram file (photo `file_path` from `getFile`).
    /// Single attempt with a generous bound (user-facing await in the
    /// sequential pump — herdr reads already take this long). Returns
    /// raw bytes; errors carry no token (the URL is built, never logged).
    pub(super) async fn download_file_impl(&self, file_path: &str) -> Res<Vec<u8>> {
        // Fail-closed: a path escaping the file host (`../`, absolute)
        // must never fetch — Telegram sends server paths, but a forged
        // update must not turn the bot into an open proxy.
        if file_path.is_empty() || file_path.starts_with('/') || file_path.contains("..") {
            return Err("refusing unsafe file path".into());
        }
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );
        let bytes = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| self.redact(&e.to_string()))?
            .error_for_status()
            .map_err(|e| self.redact(&e.to_string()))?
            .bytes()
            .await
            .map_err(|e| self.redact(&e.to_string()))?;
        // Empty body is a failed fetch, never a valid photo (an empty
        // file downstream would pose as the user's image).
        if bytes.is_empty() {
            return Err("empty file body".into());
        }
        Ok(bytes.to_vec())
    }
}
