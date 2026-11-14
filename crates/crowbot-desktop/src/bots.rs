//! How a bot and its conversations read in the window.

use crowbot_protocol::{BotState, BotSummary, Event};
use gpui::Rgba;

/// What the list says next to a bot's name. A bot with nothing to tell has no label.
pub fn status_label(summary: &BotSummary) -> Option<String> {
    match summary.state {
        BotState::Waiting => Some("Needs you".to_owned()),
        BotState::Failed => Some("Failed".to_owned()),
        BotState::Working => Some("Working…".to_owned()),
        BotState::Unread => Some(format!("{} new", summary.unread)),
        BotState::Idle => None,
    }
}

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

/// The name of the next bot made with no form: "Bot 1", "Bot 2", and so on.
pub fn next_name(bots: &[BotSummary]) -> String {
    (bots.len() + 1..)
        .map(|number| format!("Bot {number}"))
        .find(|name| bots.iter().all(|known| known.bot.name != *name))
        .unwrap_or_else(|| "Bot".to_owned())
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
    use crowbot_protocol::{Avatar, Bot, PermissionLevel, TurnEnd};

    use super::*;

    fn summary(name: &str, state: BotState, unread: u32) -> BotSummary {
        BotSummary {
            bot: Bot {
                id: 1,
                name: name.to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#3b82f6".to_owned(),
                    glyph: "A".to_owned(),
                },
                provider: "claude".to_owned(),
                model: None,
                permission_level: PermissionLevel::AskFirst,
                workspace: "/work".to_owned(),
                pinned: false,
                hidden: false,
                created_at_ms: 0,
            },
            current_conversation_id: 1,
            state,
            unread,
            last_activity_ms: 0,
        }
    }

    #[test]
    fn each_state_has_its_label_and_an_idle_bot_has_none() {
        let label = |state, unread| status_label(&summary("Atlas", state, unread));
        assert_eq!(label(BotState::Waiting, 2).as_deref(), Some("Needs you"));
        assert_eq!(label(BotState::Failed, 1).as_deref(), Some("Failed"));
        assert_eq!(label(BotState::Working, 0).as_deref(), Some("Working…"));
        assert_eq!(label(BotState::Unread, 3).as_deref(), Some("3 new"));
        assert_eq!(label(BotState::Idle, 0), None);
    }

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
            message: "gone".to_owned()
        }));
        assert!(marks_read(&Event::UserText("go".to_owned())));
    }

    #[test]
    fn a_bot_made_with_no_form_takes_the_next_free_name() {
        assert_eq!(next_name(&[]), "Bot 1");
        let bots = [
            summary("Atlas", BotState::Idle, 0),
            summary("Bot 3", BotState::Idle, 0),
        ];
        assert_eq!(next_name(&bots), "Bot 4");
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
