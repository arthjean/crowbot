//! The review of a bot's work (US-027): when the user turned the `Review`
//! origin on, the daemon starts, after enough work, a conversation of the bot
//! in which its agent reads that work and keeps what is worth keeping, with
//! the daemon's tools for memory and skills only. The review is a
//! conversation like the others, so a person reads what it did, and the
//! conversation it reviewed tells what changed.

use crowbot_protocol::Event;

use crate::handoff;

mod limits;

pub use limits::{REVIEW_OPENING, REVIEW_TOOL_CALLS, REVIEW_USER_TURNS};

/// The longest prompt of a review, in characters: the work it reads is cut
/// from its oldest end to fit.
pub const REVIEW_CHARACTERS: usize = handoff::HANDOFF_CHARACTERS;

/// What a review looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub skills: bool,
    pub memory: bool,
}

impl Focus {
    /// The review due after `tool_calls` tool calls and `user_turns` turns of
    /// a person, if any.
    pub fn due(tool_calls: u32, user_turns: u32) -> Option<Self> {
        let focus = Self {
            skills: tool_calls >= REVIEW_TOOL_CALLS,
            memory: user_turns >= REVIEW_USER_TURNS,
        };
        (focus.skills || focus.memory).then_some(focus)
    }
}

const SKILLS: &str = "Skills: if the work holds a procedure that took several steps, a correction from the person, or a technique a later task would reuse, keep it as a skill. Prefer correcting a skill you already have over writing a new one: list them with skills_list, read the one you change with skill_view, then change it with skill_manage, naming the version you read. Name a new skill by the kind of task it serves, never by one day's task.";

const MEMORY: &str = "Memory: if the person stated a preference, a fact about themselves or their work, or a correction that should hold in later conversations, keep it with the memory tool. Keep facts that last, not the state of one task, and correct an entry that is wrong rather than adding one beside it.";

const RULES: &str = "You may use only these tools of Crow Bot: memory, skills_list, skill_view, skill_manage, and session_search to read other conversations. Run no command and edit no file: every other tool is declined in a review. When nothing is worth keeping, answer \"Nothing to keep.\" and stop. Otherwise end with one line per change you made.";

/// The prompt of a review of the conversation `title`, whose work since the
/// last review is `events`.
pub fn prompt(focus: Focus, title: &str, events: &[Event]) -> String {
    let mut parts = vec![REVIEW_OPENING.to_owned()];
    parts.push(format!(
        "Read the work below, from the conversation \u{201c}{title}\u{201d}, and keep what a later conversation should know."
    ));
    if focus.skills {
        parts.push(SKILLS.to_owned());
    }
    if focus.memory {
        parts.push(MEMORY.to_owned());
    }
    parts.push(RULES.to_owned());
    let head = parts.join("\n\n");
    let opening = "\n\nThe work, oldest first:\n\n";
    let room = REVIEW_CHARACTERS.saturating_sub(head.chars().count() + opening.chars().count());
    match handoff::transcript(events, room) {
        Some(work) => format!("{head}{opening}{work}"),
        None => format!("{head}\n\nThe conversation holds no message since the last review."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_is_due_after_ten_calls_or_ten_turns_of_a_person() {
        assert_eq!(Focus::due(9, 9), None);
        assert_eq!(
            Focus::due(REVIEW_TOOL_CALLS, 0),
            Some(Focus {
                skills: true,
                memory: false
            })
        );
        assert_eq!(
            Focus::due(0, REVIEW_USER_TURNS),
            Some(Focus {
                skills: false,
                memory: true
            })
        );
    }

    #[test]
    fn a_prompt_names_its_focus_and_fits_its_limit() {
        let both = Focus {
            skills: true,
            memory: true,
        };
        let events = [
            Event::user_text("Deploy the site"),
            Event::MessageText("x".repeat(80_000)),
        ];
        let text = prompt(both, "Deploy", &events);
        assert!(text.starts_with(REVIEW_OPENING));
        assert!(text.contains(SKILLS) && text.contains(MEMORY) && text.contains(RULES));
        assert!(text.chars().count() <= REVIEW_CHARACTERS);
        let skills = prompt(
            Focus {
                skills: true,
                memory: false,
            },
            "Deploy",
            &[],
        );
        assert!(!skills.contains(MEMORY));
        assert!(skills.contains("holds no message"));
    }
}
