//! How a bot and its conversations read in the window.

use crowbot_protocol::Event;
use gpui::Rgba;

/// The color of an avatar written `#rrggbb`.
pub fn avatar_color(color: &str) -> Option<Rgba> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(gpui::rgb)
}

/// Whether an event can end a turn or ask for a person, so that a client that
/// shows it has something to mark as read. Streamed text only grows a message
/// of a turn that still runs.
pub fn marks_read(event: &Event) -> bool {
    !matches!(event, Event::MessageText(_) | Event::ThoughtText(_))
}

/// How long ago a conversation was last active, in a few words.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let minutes = (now_ms - then_ms).max(0) / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::TurnEnd;

    use super::*;

    #[test]
    fn an_avatar_color_is_read_from_its_hex_form() {
        assert_eq!(avatar_color("#3b82f6"), Some(gpui::rgb(0x3b_82_f6)));
        assert_eq!(avatar_color("3b82f6"), None);
        assert_eq!(avatar_color("#3b82"), None);
        assert_eq!(avatar_color("#zzzzzz"), None);
    }

    #[test]
    fn every_event_but_streamed_text_moves_the_read_mark() {
        assert!(!marks_read(&Event::MessageText("caw".to_owned())));
        assert!(!marks_read(&Event::ThoughtText("hm".to_owned())));
        assert!(marks_read(&Event::TurnEnded(TurnEnd::Completed)));
        assert!(marks_read(&Event::TurnFailed {
            message: "gone".to_owned(),
            cause: None,
            provider: None,
        }));
        assert!(marks_read(&Event::user_text("go".to_owned())));
    }

    #[test]
    fn the_last_activity_reads_in_a_few_words() {
        let minute = 60_000;
        assert_eq!(ago(1_000, 5_000), "just now");
        assert_eq!(ago(59_000, 0), "just now");
        assert_eq!(ago(5 * minute, 0), "5 min ago");
        assert_eq!(ago(3 * 60 * minute, 0), "3 h ago");
        assert_eq!(ago(49 * 60 * minute, 0), "2 d ago");
    }
}
