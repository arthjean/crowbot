//! The `session_search` tool: a bot searches its own past conversations,
//! from the database alone, with no model call.

use crowbot_protocol::{Event, SEARCH_MESSAGE_TERM_LENGTH, TimedEvent, cut_text};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::store::{HistorySearch, HistoryWindow, SharedStore};
use crate::tools::{ALL_KINDS, SessionKind, Tool, ToolContext, ToolFuture, TurnPermit, effect};

/// How many events before and after an event `session_search` shows.
pub const WINDOW_RADIUS: usize = 10;
/// The longest text of one event in a window, in bytes.
const WINDOW_EVENT_BYTES: usize = 2_000;

pub struct SessionSearch(pub SharedStore);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSearchInput {
    /// Words to find in your past messages, the user's, and your tool
    /// calls. Every word must match.
    #[serde(default)]
    query: Option<String>,
    /// With `event_id`: the conversation to read around that event.
    #[serde(default)]
    conversation_id: Option<i64>,
    /// With `conversation_id`: the event to read around, from a hit.
    #[serde(default)]
    event_id: Option<i64>,
}

impl Tool for SessionSearch {
    type Input = SessionSearchInput;
    type Effect = effect::Read;
    const NAME: &'static str = "session_search";
    const DESCRIPTION: &'static str = "Searches your past conversations with the user: what was said and which tools ran. Give `query` to find messages and tool calls that hold every word of it; the hits come grouped by conversation, each with its event id. Give `conversation_id` and `event_id` to read the events around one hit. The conversation you are in is not searched.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(
        &self,
        _permit: TurnPermit,
        context: ToolContext,
        input: SessionSearchInput,
    ) -> ToolFuture {
        let store = self.0.clone();
        Box::pin(async move {
            match (input.conversation_id, input.event_id, input.query) {
                (Some(conversation_id), Some(event_id), _) => {
                    window(&store, &context, conversation_id, event_id)
                }
                (None, None, Some(query)) => search(&store, &context, &query),
                _ => Err("Give `query`, or both `conversation_id` and `event_id`.".to_owned()),
            }
        })
    }
}

fn search(store: &SharedStore, context: &ToolContext, query: &str) -> Result<String, String> {
    let found = store
        .with(|store| store.search_history(context.bot_id, context.conversation_id, query))
        .map_err(|error| error.to_string())?;
    let groups = match found {
        HistorySearch::TooShort => {
            return Ok(format!(
                "No search ran: every word of the query is shorter than {SEARCH_MESSAGE_TERM_LENGTH} characters. Search with longer words."
            ));
        }
        HistorySearch::Found(groups) if groups.is_empty() => {
            return Ok(format!(
                "No past conversation holds every word of \"{}\".",
                query.trim()
            ));
        }
        HistorySearch::Found(groups) => groups,
    };
    let mut lines = Vec::new();
    for group in groups {
        lines.push(format!(
            "Conversation {} \"{}\", last active {}:",
            group.conversation_id,
            group.title.as_deref().unwrap_or("Untitled"),
            time(group.last_activity_ms)
        ));
        for hit in group.hits {
            lines.push(format!(
                "- event {} ({}, {}): {}",
                hit.event_id,
                hit.author,
                time(hit.at_ms),
                hit.snippet
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn window(
    store: &SharedStore,
    context: &ToolContext,
    conversation_id: i64,
    event_id: i64,
) -> Result<String, String> {
    let found = store
        .with(|store| {
            store.history_window(context.bot_id, conversation_id, event_id, WINDOW_RADIUS)
        })
        .map_err(|error| error.to_string())?;
    let Some(HistoryWindow { title, events }) = found else {
        return Err(format!(
            "Event {event_id} is not in conversation {conversation_id} of this bot."
        ));
    };
    let mut lines = vec![format!(
        "Conversation {conversation_id} \"{}\", around event {event_id}:",
        title.as_deref().unwrap_or("Untitled")
    )];
    lines.extend(events.iter().filter_map(|event| shown(event, event_id)));
    Ok(lines.join("\n"))
}

/// One event as a line, or `None` for one that says nothing a bot reads.
fn shown(timed: &TimedEvent, focus: i64) -> Option<String> {
    let text = match &timed.event {
        Event::UserText(message) => format!("user: {}", message.text),
        Event::MessageText(text) => format!("bot: {text}"),
        Event::ToolCallStarted { title, input, .. } => match input {
            Some(input) => format!("tool call: {title} {input}"),
            None => format!("tool call: {title}"),
        },
        Event::ToolCallUpdated {
            output: Some(output),
            ..
        } => format!("tool output: {output}"),
        Event::ApprovalResolved {
            title, decision, ..
        } => format!(
            "approval: {} {:?}",
            title.as_deref().unwrap_or("a tool call"),
            decision
        ),
        Event::TurnFailed { message, .. } => format!("the turn failed: {message}"),
        Event::MemoryChanged { changes, .. } => format!(
            "memory changed: {}",
            changes
                .iter()
                .map(|change| {
                    // As a prompt shows an entry: what the scan flags is withheld.
                    let findings = crate::scan::scan(&change.text);
                    let text = if findings.is_empty() {
                        change.text.clone()
                    } else {
                        format!("[withheld: {}]", crate::scan::describe(&findings))
                    };
                    format!("{:?} #{} {text}", change.kind, change.entry_id)
                })
                .collect::<Vec<_>>()
                .join("; ")
        ),
        _ => return None,
    };
    let mut text = text;
    if cut_text(&mut text, WINDOW_EVENT_BYTES) {
        text.push('…');
    }
    let mark = if timed.id == focus { " <-" } else { "" };
    Some(format!(
        "[event {}, {}]{mark} {text}",
        timed.id,
        time(timed.at_ms)
    ))
}

/// A time as `2026-11-24 09:30 UTC`.
pub fn time(at_ms: i64) -> String {
    let seconds = at_ms.div_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Days to a civil date, after Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_as_a_utc_date() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(time(1_795_512_600_000), "2026-11-24 09:30 UTC");
        assert_eq!(time(951_782_400_000), "2000-02-29 00:00 UTC");
    }
}
