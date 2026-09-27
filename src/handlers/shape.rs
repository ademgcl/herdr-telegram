//! `/shape [on|off]` — phone-shape final cards.
//!
//! Separate from `/transient`: that is "do I want working messages",
//! this is "make the reply readable on a phone". Default on, because an
//! unreadable reply is the complaint, not a feature.
use crate::state::AppState;

const USAGE: &str = "usage: `/shape [on|off]` — bare shows the current setting";

pub async fn handle(s: &AppState, chat: i64, thread: Option<i64>, arg: &str) {
    let t = arg.trim();
    let line = match t {
        "" => crate::ui::shape_status(s.shape_telegram()),
        "on" | "off" => {
            let on = t.eq_ignore_ascii_case("on");
            s.set_shape_telegram(on).await;
            crate::ui::shape_status(on)
        }
        _ => USAGE.to_string(),
    };
    s.tg.send_msg(chat, thread, &line, None).await;
}
