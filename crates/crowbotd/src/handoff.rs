//! What a provider reads about a conversation that ran on another provider
//! before it: the stored messages, as one text at the start of its first turn.

use crowbot_protocol::Event;

pub const HANDOFF_CHARACTERS: usize = 50_000;

const LEFT_OUT: &str = "[Earlier messages are left out.]\n\n";
const END: &str = "\n\n[End of the earlier conversation. The next message follows.]";

fn header(from: &str) -> String {
    format!(
        "This conversation ran on {from} until now. What was said before the next message follows, oldest first. Read it as context and do not answer it again.\n\n"
    )
}

enum Speaker {
    User,
    Assistant,
}

/// The text a session reads before the message of the user. `None` when
/// nothing was said. It holds `HANDOFF_CHARACTERS` at most, and what is left
/// out is the oldest.
pub fn build(events: &[Event], from: &str) -> Option<String> {
    let header = header(from);
    let fixed = header.chars().count() + END.chars().count();
    let body = transcript(events, HANDOFF_CHARACTERS.saturating_sub(fixed))?;
    Some(format!("{header}{body}{END}"))
}

/// Who said what in `events`, oldest first, in `room` characters at most:
/// what is left out is the oldest. `None` when nothing was said.
pub fn transcript(events: &[Event], room: usize) -> Option<String> {
    let mut parts: Vec<(Speaker, String)> = Vec::new();
    let mut streaming = false;
    for event in events {
        let (speaker, text) = match event {
            Event::UserText(message) => (Speaker::User, message.text.clone()),
            Event::MessageText(text) => (Speaker::Assistant, text.clone()),
            Event::ToolCallStarted { title, .. } if !title.trim().is_empty() => (
                Speaker::Assistant,
                format!("(used a tool: {})", title.trim()),
            ),
            _ => continue,
        };
        let message = matches!(event, Event::MessageText(_));
        match parts.last_mut() {
            Some((Speaker::Assistant, said)) if matches!(speaker, Speaker::Assistant) => {
                // A long answer is stored as several rows.
                if !(message && streaming) {
                    said.push('\n');
                }
                said.push_str(&text);
            }
            _ => parts.push((speaker, text)),
        }
        streaming = message;
    }
    if parts.is_empty() {
        return None;
    }
    let body = parts
        .iter()
        .map(|(speaker, said)| {
            let name = match speaker {
                Speaker::User => "User",
                Speaker::Assistant => "Assistant",
            };
            format!("{name}:\n{}", said.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let length = body.chars().count();
    Some(if length > room {
        let kept = room.saturating_sub(LEFT_OUT.chars().count());
        let tail: String = body.chars().skip(length - kept).collect();
        format!("{LEFT_OUT}{tail}")
    } else {
        body
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Event {
        Event::user_text(text.to_owned())
    }

    fn answer(text: &str) -> Event {
        Event::MessageText(text.to_owned())
    }

    #[test]
    fn nothing_said_gives_no_handoff() {
        assert_eq!(build(&[], "Claude"), None);
        assert_eq!(
            build(&[Event::ThoughtText("hm".to_owned())], "Claude"),
            None
        );
    }

    #[test]
    fn a_handoff_names_who_said_what_in_order() {
        let text = build(
            &[
                user("What is the plan?"),
                answer("The plan is "),
                answer("to ship."),
                Event::ToolCallStarted {
                    id: "call-1".to_owned(),
                    title: "Write notes.md".to_owned(),
                    status: None,
                    kind: None,
                    input: None,
                    effect: None,
                },
                answer("Done."),
                user("And then?"),
            ],
            "Claude",
        )
        .unwrap();
        assert!(text.starts_with("This conversation ran on Claude until now."));
        assert!(text.contains(
            "User:\nWhat is the plan?\n\nAssistant:\nThe plan is to ship.\n(used a tool: Write notes.md)\nDone.\n\nUser:\nAnd then?"
        ));
        assert!(text.ends_with(END));
    }

    #[test]
    fn a_long_handoff_keeps_the_most_recent_messages_within_the_limit() {
        let events = [
            user(&format!("ancient {}", "a".repeat(40_000))),
            answer(&"b".repeat(40_000)),
            user("the most recent question"),
        ];
        let text = build(&events, "Codex").unwrap();
        assert_eq!(text.chars().count(), HANDOFF_CHARACTERS);
        assert!(text.contains(LEFT_OUT));
        assert!(text.contains("User:\nthe most recent question"));
        assert!(!text.contains("ancient"));
    }
}
