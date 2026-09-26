//! `/transient [on|off]`: keep the silent working message as history
//! (off, the default) or auto-remove it when the final lands (on).
//! Global display pref — works on every prompt surface.
use crate::state::AppState;

/// Pure arg verdict (tested): bare shows status (`Ok(None)`), on/off
/// flips (`Ok(Some)`), anything else is usage (`Err` — an unknown arg
/// must never flip the flag). Single source for the handler below.
pub(crate) fn parse_transient_arg(arg: &str) -> Result<Option<bool>, ()> {
    let word = arg.split_whitespace().next().unwrap_or("");
    if word.is_empty() {
        return Ok(None);
    }
    if word.eq_ignore_ascii_case("on") {
        return Ok(Some(true));
    }
    if word.eq_ignore_ascii_case("off") {
        return Ok(Some(false));
    }
    Err(())
}

pub async fn handle_transient(s: &AppState, chat: i64, thread: Option<i64>, arg: &str) {
    match parse_transient_arg(arg) {
        Err(()) => {
            s.tg.send_msg(chat, thread, crate::ui::TRANSIENT_USAGE, None)
                .await;
            return;
        }
        Ok(flip) => {
            if let Some(on) = flip {
                s.set_transient_remove(on).await;
            }
        }
    }
    s.tg.send_msg(
        chat,
        thread,
        &crate::ui::transient_status(s.transient_remove()),
        None,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transient_arg_verdict() {
        assert_eq!(parse_transient_arg(""), Ok(None));
        assert_eq!(parse_transient_arg("on"), Ok(Some(true)));
        assert_eq!(parse_transient_arg("ON"), Ok(Some(true)));
        assert_eq!(parse_transient_arg("off"), Ok(Some(false)));
        assert_eq!(parse_transient_arg("Off"), Ok(Some(false)));
        // First word wins; unknown never flips.
        assert_eq!(parse_transient_arg("on please"), Ok(Some(true)));
        assert_eq!(parse_transient_arg("banana"), Err(()));
    }
}
