//! The reviews of a bot's work (US-027): each one is a conversation of the
//! bot, with a row that names the conversation it reviewed and the last
//! event the review counts from. What counts toward the next review is read
//! from the stored events, so it survives a restart.

use crowbot_protocol::{Event, MemoryAuthor, MemoryChange, SkillAuthor, SkillChange, UserMessage};
use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError, now_ms};

/// What has run since the last review of each kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewDue {
    /// The tool calls since the last review of skills, or the last change
    /// of a skill the bot made.
    pub tool_calls: u32,
    /// The turns of a person since the last review of memory, or the last
    /// change of memory the bot made.
    pub user_turns: u32,
}

/// One review, as its row stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRow {
    pub reviewed_conversation_id: i64,
    /// The last event of the bot when the review started.
    pub after_event_id: i64,
    pub skills: bool,
    pub memory: bool,
}

/// The conversations of a bot that are not reviews: the ones whose work a
/// review reads.
const WORK: &str =
    "c.bot_id = ?1 AND NOT EXISTS (SELECT 1 FROM reviews r WHERE r.conversation_id = c.id)";

/// A review `r` that ran its course: its turn completed, or a guard ended
/// it. A review a person's message stopped, or that failed, read nothing,
/// so the work it was given counts toward the next one, which reads it.
const RAN: &str = "EXISTS (SELECT 1 FROM events e WHERE e.conversation_id = r.conversation_id
    AND (e.kind = 'turn_interrupted' OR (e.kind = 'turn_ended' AND json_valid(e.payload)
        AND json_extract(e.payload, '$.data') = 'completed')))";

impl Store {
    /// What counts toward the next review of a bot.
    pub fn review_due(&self, bot_id: i64) -> Result<ReviewDue, StoreError> {
        let skills_mark: i64 = self.connection.query_row(
            &format!(
                "SELECT MAX(COALESCE(
                (SELECT MAX(r.after_event_id) FROM reviews r
                    WHERE r.bot_id = ?1 AND r.skills = 1 AND {RAN}), 0),
                COALESCE((SELECT MAX(e.id) FROM events e
                    JOIN conversations c ON c.id = e.conversation_id
                    WHERE c.bot_id = ?1 AND e.kind = 'skill_changed' AND json_valid(e.payload)
                        AND json_extract(e.payload, '$.data.author') IN ('bot', 'review')), 0))"
            ),
            params![bot_id],
            |row| row.get(0),
        )?;
        let memory_mark: i64 = self.connection.query_row(
            &format!(
                "SELECT MAX(COALESCE(
                (SELECT MAX(r.after_event_id) FROM reviews r
                    WHERE r.bot_id = ?1 AND r.memory = 1 AND {RAN}), 0),
                COALESCE((SELECT MAX(e.id) FROM events e
                    JOIN conversations c ON c.id = e.conversation_id
                    WHERE c.bot_id = ?1 AND e.kind = 'memory_changed' AND json_valid(e.payload)
                        AND json_extract(e.payload, '$.data.author') IN ('bot', 'review')), 0))"
            ),
            params![bot_id],
            |row| row.get(0),
        )?;
        // A tool call is counted once, however many events it started with.
        let tool_calls: i64 = self.connection.query_row(
            &format!(
                "SELECT COUNT(DISTINCT e.conversation_id || ':' ||
                    COALESCE(json_extract(e.payload, '$.data.id'), e.id))
                 FROM events e JOIN conversations c ON c.id = e.conversation_id
                 WHERE {WORK} AND e.kind = 'tool_call_started' AND e.id > ?2
                    AND json_valid(e.payload)"
            ),
            params![bot_id, skills_mark],
            |row| row.get(0),
        )?;
        let user_turns: i64 = self.connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM events e JOIN conversations c ON c.id = e.conversation_id
                 WHERE {WORK} AND e.kind = 'user_text' AND e.id > ?2 AND json_valid(e.payload)
                    AND (json_type(e.payload, '$.data') != 'object'
                        OR COALESCE(json_extract(e.payload, '$.data.origin'), 'user') = 'user')"
            ),
            params![bot_id, memory_mark],
            |row| row.get(0),
        )?;
        Ok(ReviewDue {
            tool_calls: u32::try_from(tool_calls).unwrap_or(u32::MAX),
            user_turns: u32::try_from(user_turns).unwrap_or(u32::MAX),
        })
    }

    /// Whether a person's messages or a review wait in the queue of one of
    /// the bot's conversations.
    pub fn bot_has_queued(&self, bot_id: i64) -> Result<bool, StoreError> {
        let queued = self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM queue q
                JOIN conversations c ON c.id = q.conversation_id WHERE c.bot_id = ?1)",
            params![bot_id],
            |row| row.get(0),
        )?;
        Ok(queued)
    }

    /// Starts the conversation of a review of `reviewed`, with its title. It
    /// is not the bot's current one. `None` means the conversation reviewed
    /// is gone.
    pub fn create_review(
        &self,
        reviewed: i64,
        skills: bool,
        memory: bool,
        title: &str,
    ) -> Result<Option<i64>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO conversations (bot_id, provider, cwd, title, created_at_ms)
             SELECT b.id, b.provider, b.workspace, ?2, ?3
             FROM conversations c JOIN bots b ON b.id = c.bot_id WHERE c.id = ?1",
            params![reviewed, title, now_ms()],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO reviews (conversation_id, bot_id, reviewed_conversation_id,
                after_event_id, skills, memory, created_at_ms)
             SELECT ?1, bot_id, ?2, COALESCE((SELECT MAX(id) FROM events), 0), ?3, ?4, ?5
             FROM conversations WHERE id = ?1",
            params![id, reviewed, skills, memory, now_ms()],
        )?;
        transaction.commit()?;
        Ok(Some(id))
    }

    /// The last event stored, in any conversation.
    #[cfg(test)]
    pub fn max_event_id(&self) -> Result<i64, StoreError> {
        let id =
            self.connection
                .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                    row.get(0)
                })?;
        Ok(id)
    }

    /// The review a conversation holds, when it is one.
    pub fn review(&self, conversation_id: i64) -> Result<Option<ReviewRow>, StoreError> {
        let review = self
            .connection
            .query_row(
                "SELECT reviewed_conversation_id, after_event_id, skills, memory
                 FROM reviews WHERE conversation_id = ?1",
                params![conversation_id],
                |row| {
                    Ok(ReviewRow {
                        reviewed_conversation_id: row.get(0)?,
                        after_event_id: row.get(1)?,
                        skills: row.get(2)?,
                        memory: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(review)
    }

    /// The last event of `reviewed` that an earlier review than `review`
    /// already read, to its end: the work a review reads starts after it.
    pub fn reviewed_until(&self, reviewed: i64, review: i64) -> Result<i64, StoreError> {
        let until = self.connection.query_row(
            &format!(
                "SELECT COALESCE(MAX(r.after_event_id), 0) FROM reviews r
                 WHERE r.reviewed_conversation_id = ?1 AND r.conversation_id != ?2 AND {RAN}"
            ),
            params![reviewed, review],
            |row| row.get(0),
        )?;
        Ok(until)
    }

    /// The messages and the tool calls of a conversation after the event
    /// `after`, up to `until`, oldest first. The most recent ones are kept,
    /// up to the one that brings their text past `characters`.
    pub fn work_events(
        &self,
        conversation_id: i64,
        after: i64,
        until: i64,
        characters: usize,
    ) -> Result<Vec<Event>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM events
             WHERE conversation_id = ?1 AND id > ?2 AND id <= ?3
                AND kind IN ('user_text', 'message_text', 'tool_call_started')
             ORDER BY id DESC",
        )?;
        let mut rows = statement.query(params![conversation_id, after, until])?;
        let mut events = Vec::new();
        let mut kept = 0;
        while kept < characters
            && let Some(row) = rows.next()?
        {
            let event: Event = serde_json::from_str(&row.get::<_, String>(0)?)?;
            kept += match &event {
                Event::UserText(UserMessage { text, .. }) | Event::MessageText(text) => {
                    text.chars().count()
                }
                Event::ToolCallStarted { title, .. } => title.chars().count(),
                _ => 0,
            };
            events.push(event);
        }
        events.reverse();
        Ok(events)
    }

    /// What a review changed: the memory and the skills its agent changed,
    /// in its own conversation.
    pub fn review_changes(
        &self,
        review: i64,
    ) -> Result<(Vec<MemoryChange>, Vec<SkillChange>), StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM events
             WHERE conversation_id = ?1 AND kind IN ('memory_changed', 'skill_changed')
             ORDER BY id",
        )?;
        let payloads = statement
            .query_map(params![review], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut memory = Vec::new();
        let mut skills = Vec::new();
        for payload in payloads {
            match serde_json::from_str(&payload)? {
                Event::MemoryChanged {
                    author: MemoryAuthor::Review,
                    changes,
                } => memory.extend(changes),
                Event::SkillChanged {
                    author: SkillAuthor::Review,
                    changes,
                } => skills.extend(changes),
                _ => {}
            }
        }
        Ok((memory, skills))
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{SkillChangeKind, ToolStatus, TurnEnd, TurnOrigin};

    use super::*;
    use crate::store::NewBot;

    fn store() -> (Store, i64, i64) {
        let store =
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let bot = store
            .create_bot(&NewBot {
                name: "Atlas".to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: crowbot_protocol::Avatar {
                    color: "#3b82f6".to_owned(),
                    shape: crowbot_protocol::MascotShape::Smooth,
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: crowbot_protocol::PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/".to_owned(),
            })
            .unwrap();
        (store, bot.bot.id, bot.current_conversation_id)
    }

    fn call(store: &Store, conversation: i64, id: &str) {
        for status in [ToolStatus::InProgress, ToolStatus::Completed] {
            store
                .append(
                    conversation,
                    &Event::ToolCallStarted {
                        id: id.to_owned(),
                        title: "Read".to_owned(),
                        status: Some(status),
                        kind: None,
                        input: None,
                        effect: None,
                    },
                )
                .unwrap();
        }
    }

    fn skill_change(author: SkillAuthor) -> Event {
        Event::SkillChanged {
            author,
            changes: vec![SkillChange {
                name: "deploy".to_owned(),
                kind: SkillChangeKind::Created,
                old_hash: None,
                new_hash: Some("ab".to_owned()),
            }],
        }
    }

    #[test]
    fn calls_and_turns_count_from_the_last_review_or_change_of_the_bot() {
        let (store, bot, work) = store();
        store.append(work, &Event::user_text("go")).unwrap();
        store
            .append(
                work,
                &Event::UserText(UserMessage::new("again").with_origin(TurnOrigin::Retry)),
            )
            .unwrap();
        call(&store, work, "a");
        call(&store, work, "b");
        assert_eq!(
            store.review_due(bot).unwrap(),
            ReviewDue {
                tool_calls: 2,
                user_turns: 1
            }
        );
        // A person's change of a skill is not a call of skill_manage.
        store
            .append(work, &skill_change(SkillAuthor::User))
            .unwrap();
        assert_eq!(store.review_due(bot).unwrap().tool_calls, 2);
        store.append(work, &skill_change(SkillAuthor::Bot)).unwrap();
        call(&store, work, "c");
        assert_eq!(store.review_due(bot).unwrap().tool_calls, 1);

        let review = store
            .create_review(work, true, false, "Review")
            .unwrap()
            .unwrap();
        assert_eq!(
            store.bot(bot).unwrap().unwrap().current_conversation_id,
            work
        );
        // What a review does is not work to review.
        call(&store, review, "d");
        store
            .append(review, &skill_change(SkillAuthor::Review))
            .unwrap();
        store
            .append(review, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
        assert_eq!(
            store.review_due(bot).unwrap(),
            ReviewDue {
                tool_calls: 0,
                user_turns: 1
            }
        );
        let row = store.review(review).unwrap().unwrap();
        assert_eq!(row.reviewed_conversation_id, work);
        assert!(row.skills && !row.memory);
        assert_eq!(store.review(work).unwrap(), None);
        let (memory, skills) = store.review_changes(review).unwrap();
        assert!(memory.is_empty());
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(store.reviewed_until(work, review).unwrap(), 0);
    }

    #[test]
    fn a_review_stopped_or_failed_leaves_its_work_to_the_next_one() {
        let (store, bot, work) = store();
        for id in ["a", "b", "c"] {
            call(&store, work, id);
        }
        let ends = [
            Event::TurnEnded(TurnEnd::Cancelled),
            Event::TurnFailed {
                message: "gone".to_owned(),
                cause: None,
                provider: Some("claude".to_owned()),
            },
        ];
        for end in ends {
            let review = store
                .create_review(work, true, true, "Review")
                .unwrap()
                .unwrap();
            store.append(review, &Event::user_text("review")).unwrap();
            store.append(review, &end).unwrap();
            assert_eq!(
                store.review_due(bot).unwrap().tool_calls,
                3,
                "after {end:?}"
            );
            assert_eq!(store.reviewed_until(work, review + 1).unwrap(), 0);
        }
        // A review that ran to its end, even one a guard ended, read it.
        let review = store
            .create_review(work, true, true, "Review")
            .unwrap()
            .unwrap();
        let mark = store.review(review).unwrap().unwrap().after_event_id;
        store.append(review, &Event::user_text("review")).unwrap();
        store
            .append(
                review,
                &Event::TurnInterrupted {
                    cause: crowbot_protocol::Interruption::BudgetExhausted,
                },
            )
            .unwrap();
        assert_eq!(store.review_due(bot).unwrap(), ReviewDue::default());
        assert_eq!(store.reviewed_until(work, review + 1).unwrap(), mark);
    }

    #[test]
    fn the_work_a_review_reads_ends_where_it_started() {
        let (store, _, work) = store();
        store.append(work, &Event::user_text("first")).unwrap();
        store
            .append(work, &Event::MessageText("done".to_owned()))
            .unwrap();
        let until = store.max_event_id().unwrap();
        store.append(work, &Event::user_text("later")).unwrap();
        let events = store.work_events(work, 0, until, 1_000).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(store.work_events(work, until, until, 1_000).unwrap(), []);
    }
}
