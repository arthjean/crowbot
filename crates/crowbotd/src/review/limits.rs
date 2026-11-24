//! The figures of the review of a bot's work. The daemon's tests read this
//! file too, so that a test and the code share each figure.

/// The tool calls since the last review of skills, or the last change of a
/// skill the bot made, after which a review is due
/// (`creation_nudge_interval`, `hermes-agent/agent/agent_init.py:1221`).
pub const REVIEW_TOOL_CALLS: u32 = 10;
/// The turns of a person since the last review of memory, or the last change
/// of memory the bot made, after which a review that may write memory is due
/// (`nudge_interval`, `hermes-agent/agent/agent_init.py:1177`).
pub const REVIEW_USER_TURNS: u32 = 10;
/// How every review prompt opens.
pub const REVIEW_OPENING: &str = "You are reviewing your recent work for Crow Bot.";
