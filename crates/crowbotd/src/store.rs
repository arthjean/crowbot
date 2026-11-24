mod bots;
mod preferences;
mod search;
mod sessions;
mod usage;

pub use bots::{BotRow, NewBot};
pub use sessions::{ProviderSession, ProviderSetting, session_fingerprint};
pub use usage::{Reported, Tokens};

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::{
    ApprovalClosure, ConversationSummary, Event, Interruption, QueueState, QueuedMessage,
    TimedEvent, UserMessage,
};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA_VERSION: i64 = 10;
const TITLE_LENGTH: usize = 80;

/// Streamed text waits at most this long in memory before it reaches the database.
const SAVE_INTERVAL: Duration = Duration::from_millis(250);
/// Streamed text that grew by this much is saved without waiting for the interval.
const SAVE_BYTES: usize = 8 << 10;
/// A text row that reached this size is closed, and the stream goes on in a new row.
const ROW_BYTES: usize = 64 << 10;

const SCHEMA_V1: &str = "
BEGIN;
CREATE TABLE conversations (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    command TEXT,
    cwd TEXT NOT NULL,
    provider_session_id TEXT,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX events_by_conversation ON events(conversation_id, id);
PRAGMA user_version = 1;
COMMIT;
";

const SCHEMA_V2: &str = "
ALTER TABLE conversations ADD COLUMN queue_paused INTEGER NOT NULL DEFAULT 0;
CREATE TABLE queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    position INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX queue_by_conversation ON queue(conversation_id, position, id);
";

/// Bots own the conversations. The columns `provider`, `command`, and `cwd` of a
/// conversation stay as a record of where it started: the daemon reads the
/// provider and the folder from the bot.
const SCHEMA_V3: &str = "
CREATE TABLE bots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL DEFAULT '',
    avatar_color TEXT NOT NULL,
    avatar_glyph TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    permission_level TEXT NOT NULL DEFAULT 'ask_first',
    workspace TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    current_conversation_id INTEGER,
    session_revision INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL
);
ALTER TABLE conversations ADD COLUMN bot_id INTEGER REFERENCES bots(id);
ALTER TABLE conversations ADD COLUMN title TEXT;
ALTER TABLE conversations ADD COLUMN session_revision INTEGER;
ALTER TABLE conversations ADD COLUMN read_event_id INTEGER NOT NULL DEFAULT 0;
CREATE INDEX conversations_by_bot ON conversations(bot_id);
";

const SCHEMA_V4: &str = "
CREATE TABLE preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

const SCHEMA_V5: &str = "
ALTER TABLE conversations ADD COLUMN draft TEXT NOT NULL DEFAULT '';
";

const SCHEMA_V6: &str = "
CREATE VIRTUAL TABLE event_text USING fts5(
    body,
    tokenize = 'trigram remove_diacritics 1',
    content = '',
    contentless_delete = 1
);
CREATE TRIGGER event_text_insert AFTER INSERT ON events
WHEN new.kind IN ('user_text', 'message_text') AND json_valid(new.payload)
BEGIN
    INSERT INTO event_text (rowid, body) VALUES (new.id, json_extract(new.payload, '$.data'));
END;
CREATE TRIGGER event_text_update AFTER UPDATE OF payload ON events
WHEN new.kind IN ('user_text', 'message_text')
BEGIN
    DELETE FROM event_text WHERE rowid = old.id;
    INSERT INTO event_text (rowid, body)
        SELECT new.id, json_extract(new.payload, '$.data') WHERE json_valid(new.payload);
END;
CREATE TRIGGER event_text_delete AFTER DELETE ON events
WHEN old.kind IN ('user_text', 'message_text')
BEGIN
    DELETE FROM event_text WHERE rowid = old.id;
END;
INSERT INTO event_text (rowid, body)
    SELECT id, json_extract(payload, '$.data') FROM events
    WHERE kind IN ('user_text', 'message_text') AND json_valid(payload);
";

/// A conversation holds one provider session for each provider that ran it.
/// The columns `provider_session_id` and `session_revision` of a conversation
/// stay as they were: the daemon no longer reads them.
const SCHEMA_V7: &str = "
ALTER TABLE bots ADD COLUMN effort TEXT;
CREATE TABLE provider_sessions (
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    provider TEXT NOT NULL,
    session_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    last_event_id INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (conversation_id, provider)
);
CREATE TABLE provider_settings (
    provider TEXT PRIMARY KEY,
    executable TEXT,
    signed_in INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE bot_models (
    bot_id INTEGER NOT NULL REFERENCES bots(id),
    provider TEXT NOT NULL,
    model TEXT,
    effort TEXT,
    PRIMARY KEY (bot_id, provider)
);
";

/// A bot is drawn as a mascot, with a head the user picks. The column
/// `avatar_glyph` of a bot stays as it was: the daemon no longer reads it.
const SCHEMA_V8: &str = "
ALTER TABLE bots ADD COLUMN avatar_shape TEXT NOT NULL DEFAULT 'smooth';
";

/// A provider session keeps the version of each section of the system
/// prompt it has seen, at its start and in the notes since, as a JSON object.
const SCHEMA_V9: &str = "
ALTER TABLE provider_sessions ADD COLUMN sections TEXT NOT NULL DEFAULT '{}';
";

/// A bot may have a limit on the tool calls of a turn. What an agent reports
/// a turn consumed is kept with the event that started the turn. The search
/// index holds the text of a message that is stored with a note or an
/// origin, and no longer the JSON around it.
const SCHEMA_V10: &str = "
ALTER TABLE bots ADD COLUMN tool_call_limit INTEGER;
CREATE TABLE turn_usage (
    turn_event_id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id),
    provider TEXT NOT NULL,
    session_id TEXT,
    context_used INTEGER,
    context_size INTEGER,
    session_cost REAL,
    turn_cost REAL,
    cost_currency TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX turn_usage_by_conversation ON turn_usage(conversation_id, turn_event_id);
DROP TRIGGER event_text_insert;
DROP TRIGGER event_text_update;
CREATE TRIGGER event_text_insert AFTER INSERT ON events
WHEN new.kind IN ('user_text', 'message_text') AND json_valid(new.payload)
BEGIN
    INSERT INTO event_text (rowid, body) VALUES (new.id,
        COALESCE(json_extract(new.payload, '$.data.text'), json_extract(new.payload, '$.data')));
END;
CREATE TRIGGER event_text_update AFTER UPDATE OF payload ON events
WHEN new.kind IN ('user_text', 'message_text')
BEGIN
    DELETE FROM event_text WHERE rowid = old.id;
    INSERT INTO event_text (rowid, body)
        SELECT new.id,
            COALESCE(json_extract(new.payload, '$.data.text'), json_extract(new.payload, '$.data'))
        WHERE json_valid(new.payload);
END;
DELETE FROM event_text WHERE rowid IN (
    SELECT id FROM events
    WHERE kind = 'user_text' AND json_valid(payload) AND json_type(payload, '$.data') = 'object'
);
INSERT INTO event_text (rowid, body)
    SELECT id, json_extract(payload, '$.data.text') FROM events
    WHERE kind = 'user_text' AND json_valid(payload) AND json_type(payload, '$.data') = 'object';
";

const UNFINISHED_TURNS: &str = "
SELECT turn.conversation_id, turn.started
FROM (
    SELECT conversation_id, MAX(id) AS started
    FROM events
    WHERE kind = 'user_text'
    GROUP BY conversation_id
) turn
WHERE NOT EXISTS (
    SELECT 1 FROM events
    WHERE conversation_id = turn.conversation_id
        AND id > turn.started
        AND kind IN ('turn_ended', 'turn_failed', 'turn_interrupted')
)
";

const SUMMARIES: &str = "
SELECT
    c.id,
    c.bot_id,
    c.title,
    c.created_at_ms,
    COALESCE((SELECT MAX(created_at_ms) FROM events WHERE conversation_id = c.id), c.created_at_ms)
FROM conversations c
WHERE (?1 IS NULL OR c.bot_id = ?1) AND (?2 IS NULL OR c.id = ?2)
ORDER BY 5 DESC, c.id DESC
";

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    NewerSchema(i64),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "database error: {error}"),
            Self::Json(error) => write!(formatter, "stored event is not valid: {error}"),
            Self::Io(error) => write!(formatter, "cannot open the database: {error}"),
            Self::NewerSchema(version) => write!(
                formatter,
                "the database has schema version {version}, newer than this daemon supports ({SCHEMA_VERSION})"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct Conversation {
    pub id: i64,
    pub bot_id: i64,
    pub title: Option<String>,
}

pub struct EventsPage {
    pub events: Vec<TimedEvent>,
    pub older: Option<i64>,
}

/// A part of a conversation that may end before its last event.
pub struct EventsWindow {
    pub events: Vec<TimedEvent>,
    pub older: Option<i64>,
    pub newer: Option<i64>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::NewerSchema(version));
        }
        if version < 1 {
            self.connection.execute_batch(SCHEMA_V1)?;
        }
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        // Every pending step and the version number are applied together, or
        // nothing at all: a failure leaves the database as it was.
        let transaction = self.connection.unchecked_transaction()?;
        if version < 2 {
            transaction.execute_batch(SCHEMA_V2)?;
        }
        if version < 3 {
            transaction.execute_batch(SCHEMA_V3)?;
            self.adopt_conversations()?;
        }
        if version < 4 {
            transaction.execute_batch(SCHEMA_V4)?;
        }
        if version < 5 {
            transaction.execute_batch(SCHEMA_V5)?;
        }
        if version < 6 {
            transaction.execute_batch(SCHEMA_V6)?;
        }
        if version < 7 {
            transaction.execute_batch(SCHEMA_V7)?;
            self.adopt_sessions()?;
        }
        if version < 8 {
            transaction.execute_batch(SCHEMA_V8)?;
            self.assign_mascots()?;
        }
        if version < 9 {
            transaction.execute_batch(SCHEMA_V9)?;
        }
        if version < 10 {
            transaction.execute_batch(SCHEMA_V10)?;
        }
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    /// Closes what a daemon that stopped without warning left open: each turn that
    /// was running ends as interrupted, each approval that waited expires, and each
    /// queue that still holds messages pauses until a person resumes it.
    pub fn recover(&self) -> Result<usize, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let unfinished = {
            let mut statement = transaction.prepare(UNFINISHED_TURNS)?;
            statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (conversation_id, started) in &unfinished {
            for (id, tool_call_id, title) in self.open_approvals(*conversation_id, *started)? {
                self.append(
                    *conversation_id,
                    &Event::ApprovalClosed {
                        id,
                        tool_call_id,
                        title,
                        reason: ApprovalClosure::Expired,
                    },
                )?;
            }
            self.append(
                *conversation_id,
                &Event::TurnInterrupted {
                    cause: Interruption::DaemonRestarted,
                },
            )?;
        }
        transaction.execute(
            "UPDATE conversations SET queue_paused = 1 WHERE id IN (SELECT conversation_id FROM queue)",
            [],
        )?;
        transaction.commit()?;
        Ok(unfinished.len())
    }

    fn open_approvals(
        &self,
        conversation_id: i64,
        after: i64,
    ) -> Result<Vec<(String, String, Option<String>)>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM events
             WHERE conversation_id = ?1 AND id > ?2
                AND kind IN ('approval_requested', 'approval_resolved', 'approval_closed')
             ORDER BY id",
        )?;
        let payloads = statement
            .query_map(params![conversation_id, after], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut open = Vec::new();
        for payload in payloads {
            match serde_json::from_str(&payload)? {
                Event::ApprovalRequested {
                    id,
                    tool_call_id,
                    title,
                    ..
                } => open.push((id, tool_call_id, title)),
                Event::ApprovalResolved {
                    id: Some(settled), ..
                }
                | Event::ApprovalClosed { id: settled, .. } => {
                    open.retain(|(id, ..)| *id != settled);
                }
                _ => {}
            }
        }
        Ok(open)
    }

    pub fn conversation(&self, id: i64) -> Result<Option<Conversation>, StoreError> {
        let conversation = self
            .connection
            .query_row(
                "SELECT id, bot_id, title FROM conversations WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Conversation {
                        id: row.get(0)?,
                        bot_id: row.get(1)?,
                        title: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(conversation)
    }

    /// The conversations of a bot, the most recently active first.
    pub fn summaries(&self, bot_id: i64) -> Result<Vec<ConversationSummary>, StoreError> {
        self.query_summaries(Some(bot_id), None)
    }

    pub fn summary(&self, id: i64) -> Result<Option<ConversationSummary>, StoreError> {
        Ok(self.query_summaries(None, Some(id))?.pop())
    }

    fn query_summaries(
        &self,
        bot_id: Option<i64>,
        id: Option<i64>,
    ) -> Result<Vec<ConversationSummary>, StoreError> {
        let mut statement = self.connection.prepare(SUMMARIES)?;
        let summaries = statement
            .query_map(params![bot_id, id], |row| {
                Ok(ConversationSummary {
                    id: row.get(0)?,
                    bot_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at_ms: row.get(3)?,
                    last_activity_ms: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(summaries)
    }

    /// Gives a conversation its title, from its first message, when it has none.
    pub fn set_title(&self, conversation_id: i64, first_message: &str) -> Result<bool, StoreError> {
        let Some(title) = title_of(first_message) else {
            return Ok(false);
        };
        let titled = self.connection.execute(
            "UPDATE conversations SET title = ?2 WHERE id = ?1 AND title IS NULL",
            params![conversation_id, title],
        )?;
        Ok(titled > 0)
    }

    /// Applies a change to the bot's own state and stores the event it
    /// records, in one transaction: a change and its event are kept or lost
    /// together.
    pub fn apply_mutation(
        &self,
        conversation_id: i64,
        mutation: crate::tools::Mutation,
    ) -> Result<crate::tools::Applied, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let applied = mutation.apply(self)?;
        if let Some(event) = &applied.event {
            self.append(conversation_id, event)?;
        }
        transaction.commit()?;
        Ok(applied)
    }

    pub fn append(&self, conversation_id: i64, event: &Event) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![
                conversation_id,
                kind(event),
                serde_json::to_string(event)?,
                now_ms()
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    fn rewrite(&self, row: i64, event: &Event) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE events SET payload = ?2 WHERE id = ?1",
            params![row, serde_json::to_string(event)?],
        )?;
        Ok(())
    }

    pub fn queue(&self, conversation_id: i64) -> Result<QueueState, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, text FROM queue WHERE conversation_id = ?1 ORDER BY position, id",
        )?;
        let messages = statement
            .query_map(params![conversation_id], |row| {
                Ok(QueuedMessage {
                    id: row.get(0)?,
                    text: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let paused = self
            .connection
            .query_row(
                "SELECT queue_paused FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        Ok(QueueState {
            paused: paused && !messages.is_empty(),
            messages,
        })
    }

    pub fn enqueue(&self, conversation_id: i64, text: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO queue (conversation_id, position, text, created_at_ms)
             VALUES (?1, COALESCE((SELECT MAX(position) FROM queue WHERE conversation_id = ?1), 0), ?2, ?3)",
            params![conversation_id, text, now_ms()],
        )?;
        Ok(())
    }

    pub fn edit_queued(
        &self,
        conversation_id: i64,
        queue_id: i64,
        text: &str,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE queue SET text = ?3 WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, queue_id, text],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_queued(&self, conversation_id: i64, queue_id: i64) -> Result<bool, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let deleted = transaction.execute(
            "DELETE FROM queue WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, queue_id],
        )?;
        self.unpause_when_empty(conversation_id)?;
        transaction.commit()?;
        Ok(deleted > 0)
    }

    pub fn move_queued_first(
        &self,
        conversation_id: i64,
        queue_id: i64,
    ) -> Result<bool, StoreError> {
        let moved = self.connection.execute(
            "UPDATE queue
             SET position = (SELECT MIN(position) FROM queue WHERE conversation_id = ?1) - 1
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id, queue_id],
        )?;
        Ok(moved > 0)
    }

    pub fn set_queue_paused(&self, conversation_id: i64, paused: bool) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE conversations SET queue_paused = ?2 WHERE id = ?1",
            params![conversation_id, paused],
        )?;
        Ok(())
    }

    /// Takes the first queued message out of the queue and records it as the user
    /// message of a new turn. Both happen together, so a message that left the
    /// queue is in the conversation, and a daemon that stops in between starts
    /// nothing twice.
    pub fn start_queued(
        &self,
        conversation_id: i64,
        note: Option<String>,
    ) -> Result<Option<String>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let first = transaction
            .query_row(
                "SELECT id, text FROM queue WHERE conversation_id = ?1 ORDER BY position, id LIMIT 1",
                params![conversation_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((queue_id, text)) = first else {
            return Ok(None);
        };
        transaction.execute("DELETE FROM queue WHERE id = ?1", params![queue_id])?;
        let message = UserMessage::new(text.clone()).with_note(note);
        self.append(conversation_id, &Event::UserText(message))?;
        self.unpause_when_empty(conversation_id)?;
        transaction.commit()?;
        Ok(Some(text))
    }

    fn unpause_when_empty(&self, conversation_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE conversations SET queue_paused = 0
             WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM queue WHERE conversation_id = ?1)",
            params![conversation_id],
        )?;
        Ok(())
    }

    pub fn events_page(
        &self,
        conversation_id: i64,
        before: Option<i64>,
        limit: usize,
    ) -> Result<EventsPage, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, payload, created_at_ms FROM events
             WHERE conversation_id = ?1 AND id < ?2
             ORDER BY id DESC LIMIT ?3",
        )?;
        let wanted = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let mut rows = statement
            .query_map(
                params![conversation_id, before.unwrap_or(i64::MAX), wanted],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        rows.reverse();
        let older = rows.first().map(|(id, ..)| *id).filter(|_| more);
        Ok(EventsPage {
            events: timed(rows)?,
            older,
        })
    }

    /// The events that show a stored event to a client that holds the ones
    /// from `before` on. When at most `joined` events lie between the start of
    /// the event's turn and `before`, all of them come and `newer` is empty.
    /// Otherwise `page` events around the event come, and `newer` names the
    /// last of them.
    pub fn events_around(
        &self,
        conversation_id: i64,
        event_id: i64,
        before: i64,
        joined: usize,
        page: usize,
    ) -> Result<EventsWindow, StoreError> {
        let turn: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(id), ?2) FROM events
             WHERE conversation_id = ?1 AND id <= ?2 AND kind = 'user_text'",
            params![conversation_id, event_id],
            |row| row.get(0),
        )?;
        let rows = self.event_rows(conversation_id, turn, before, joined.saturating_add(1))?;
        let (rows, newer) = if rows.len() <= joined {
            (rows, None)
        } else {
            let lead = i64::try_from(page / 2).unwrap_or(i64::MAX);
            let from: i64 = self.connection.query_row(
                "SELECT COALESCE(MIN(id), ?2) FROM (
                     SELECT id FROM events
                     WHERE conversation_id = ?1 AND id >= ?2 AND id <= ?3
                     ORDER BY id DESC LIMIT ?4)",
                params![conversation_id, turn, event_id, lead],
                |row| row.get(0),
            )?;
            let mut rows =
                self.event_rows(conversation_id, from, before, page.saturating_add(1))?;
            let more = rows.len() > page;
            rows.truncate(page);
            let newer = rows.last().map(|(id, ..)| *id).filter(|_| more);
            (rows, newer)
        };
        let lowest = rows.first().map_or(before, |(id, ..)| *id);
        let more: bool = self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM events WHERE conversation_id = ?1 AND id < ?2)",
            params![conversation_id, lowest],
            |row| row.get(0),
        )?;
        Ok(EventsWindow {
            events: timed(rows)?,
            older: more.then_some(lowest),
            newer,
        })
    }

    /// At most `limit` events after `after` and before `until`. `newer` names
    /// the last of them when more lie before `until`.
    pub fn events_between(
        &self,
        conversation_id: i64,
        after: i64,
        until: i64,
        limit: usize,
    ) -> Result<EventsWindow, StoreError> {
        let from = after.saturating_add(1);
        let mut rows = self.event_rows(conversation_id, from, until, limit.saturating_add(1))?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        let newer = rows.last().map(|(id, ..)| *id).filter(|_| more);
        Ok(EventsWindow {
            events: timed(rows)?,
            older: None,
            newer,
        })
    }

    fn event_rows(
        &self,
        conversation_id: i64,
        from: i64,
        before: i64,
        limit: usize,
    ) -> Result<Vec<(i64, String, i64)>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, payload, created_at_ms FROM events
             WHERE conversation_id = ?1 AND id >= ?2 AND id < ?3
             ORDER BY id LIMIT ?4",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(params![conversation_id, from, before, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn draft(&self, conversation_id: i64) -> Result<String, StoreError> {
        let draft = self
            .connection
            .query_row(
                "SELECT draft FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(draft.unwrap_or_default())
    }

    pub fn set_draft(&self, conversation_id: i64, text: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE conversations SET draft = ?2 WHERE id = ?1 AND draft IS NOT ?2",
            params![conversation_id, text],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn events(&self, conversation_id: i64) -> Result<Vec<Event>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT payload FROM events WHERE conversation_id = ?1 ORDER BY id")?;
        let payloads = statement
            .query_map(params![conversation_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        payloads
            .iter()
            .map(|payload| serde_json::from_str(payload).map_err(StoreError::from))
            .collect()
    }
}

fn timed(rows: Vec<(i64, String, i64)>) -> Result<Vec<TimedEvent>, StoreError> {
    rows.into_iter()
        .map(|(id, payload, at_ms)| {
            Ok(TimedEvent {
                id,
                at_ms,
                event: serde_json::from_str(&payload)?,
            })
        })
        .collect()
}

fn kind(event: &Event) -> &'static str {
    match event {
        Event::UserText(_) => "user_text",
        Event::MessageText(_) => "message_text",
        Event::ThoughtText(_) => "thought_text",
        Event::ToolCallStarted { .. } => "tool_call_started",
        Event::ToolCallUpdated { .. } => "tool_call_updated",
        Event::ApprovalRequested { .. } => "approval_requested",
        Event::ApprovalResolved { .. } => "approval_resolved",
        Event::ApprovalClosed { .. } => "approval_closed",
        Event::TurnEnded(_) => "turn_ended",
        Event::TurnFailed { .. } => "turn_failed",
        Event::TurnInterrupted { .. } => "turn_interrupted",
        Event::ModelUnavailable { .. } => "model_unavailable",
        Event::ProviderChanged { .. } => "provider_changed",
        Event::ToolRefused { .. } => "tool_refused",
    }
}

/// The title a conversation takes from its first message: the first line that
/// holds text, cut at 80 characters.
fn title_of(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut title: String = line.chars().take(TITLE_LENGTH).collect();
    if line.chars().count() > TITLE_LENGTH {
        title.push('…');
    }
    Some(title)
}

#[derive(Clone)]
pub struct SharedStore(Arc<Mutex<Store>>);

impl SharedStore {
    pub fn new(store: Store) -> Self {
        Self(Arc::new(Mutex::new(store)))
    }

    pub fn with<R>(&self, read_or_write: impl FnOnce(&Store) -> R) -> R {
        let store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        read_or_write(&store)
    }

    /// Closes the database, for a daemon that stops: SQLite moves what the
    /// write-ahead log holds into the database and removes the `-wal` and
    /// `-shm` files. A use of the store afterward fails, on an empty
    /// database in memory.
    pub fn close(&self) -> Result<(), StoreError> {
        let mut store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let closed = std::mem::replace(&mut store.connection, Connection::open_in_memory()?);
        closed.close().map_err(|(_, error)| StoreError::from(error))
    }
}

/// Writes the events of one conversation. Streamed text grows one row in place:
/// the row is saved on a short interval, so a daemon that stops without warning
/// loses a fraction of a second of text at most.
pub struct Recorder {
    store: SharedStore,
    conversation_id: i64,
    open: Option<OpenText>,
}

struct OpenText {
    event: Event,
    row: Option<i64>,
    unsaved: usize,
    unsaved_since: Option<Instant>,
}

impl Recorder {
    pub fn new(store: SharedStore, conversation_id: i64) -> Self {
        Self {
            store,
            conversation_id,
            open: None,
        }
    }

    pub fn record(&mut self, event: Event) -> Result<(), StoreError> {
        let chunk = match &event {
            Event::MessageText(text) | Event::ThoughtText(text) => text.len(),
            _ => {
                self.close()?;
                return self.append(&event).map(|_| ());
            }
        };
        let grown = match (self.open.as_mut().map(|open| &mut open.event), &event) {
            (Some(Event::MessageText(buffer)), Event::MessageText(text))
            | (Some(Event::ThoughtText(buffer)), Event::ThoughtText(text)) => {
                buffer.push_str(text);
                Some(buffer.len())
            }
            _ => None,
        };
        let size = match grown {
            Some(size) => size,
            None => {
                self.close()?;
                self.open = Some(OpenText {
                    event,
                    row: None,
                    unsaved: 0,
                    unsaved_since: None,
                });
                chunk
            }
        };
        if let Some(open) = &mut self.open {
            open.unsaved += chunk;
            open.unsaved_since.get_or_insert_with(Instant::now);
            if size >= ROW_BYTES {
                return self.close();
            }
            if open.unsaved >= SAVE_BYTES {
                return self.save();
            }
        }
        Ok(())
    }

    /// When the text still in memory must reach the database.
    pub fn save_due(&self) -> Option<Instant> {
        let since = self.open.as_ref()?.unsaved_since?;
        Some(since + SAVE_INTERVAL)
    }

    /// Writes the text received so far and keeps its row open for what follows.
    pub fn save(&mut self) -> Result<(), StoreError> {
        let Some(open) = &mut self.open else {
            return Ok(());
        };
        if open.unsaved_since.take().is_none() {
            return Ok(());
        }
        open.unsaved = 0;
        let conversation_id = self.conversation_id;
        match open.row {
            Some(row) => self.store.with(|store| store.rewrite(row, &open.event)),
            None => {
                let row = self
                    .store
                    .with(|store| store.append(conversation_id, &open.event))?;
                open.row = Some(row);
                Ok(())
            }
        }
    }

    /// Writes the text received so far and ends its row: later text starts a new one.
    pub fn close(&mut self) -> Result<(), StoreError> {
        let saved = self.save();
        self.open = None;
        saved
    }

    fn append(&self, event: &Event) -> Result<i64, StoreError> {
        self.store
            .with(|store| store.append(self.conversation_id, event))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{ToolStatus, TurnEnd};

    use super::*;

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn a_mutation_and_its_event_are_kept_or_lost_together() {
        use crate::tools::{Applied, TurnGate};
        let store = store();
        let id = conversation(&store);
        let gate = TurnGate::default();
        gate.open(1);
        let permit = gate.permit().unwrap();
        let kept = permit.mutation(move |store| {
            store.set_draft(id, "kept")?;
            Ok(Applied {
                output: "done".to_owned(),
                event: Some(Event::MessageText("changed".to_owned())),
            })
        });
        assert_eq!(store.apply_mutation(id, kept).unwrap().output, "done");
        assert_eq!(
            store.events(id).unwrap(),
            vec![Event::MessageText("changed".to_owned())]
        );
        let failed = permit.mutation(move |store| {
            store.set_draft(id, "lost")?;
            Err(StoreError::NewerSchema(99))
        });
        assert!(store.apply_mutation(id, failed).is_err());
        assert_eq!(store.events(id).unwrap().len(), 1);
        assert_eq!(store.draft(id).unwrap(), "kept");
    }

    fn bot(store: &Store) -> bots::BotRow {
        store
            .create_bot(&bots::NewBot {
                name: "Atlas".to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: crowbot_protocol::Avatar {
                    color: "#3b82f6".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: crowbot_protocol::PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/work".to_owned(),
            })
            .unwrap()
    }

    /// A conversation of a bot of its own.
    fn conversation(store: &Store) -> i64 {
        bot(store).current_conversation_id
    }

    fn events(store: &SharedStore, id: i64) -> Vec<Event> {
        store.with(|store| store.events(id)).unwrap()
    }

    #[test]
    fn events_come_back_in_the_order_they_were_appended() {
        let store = store();
        let id = conversation(&store);
        let events = vec![
            Event::user_text("hello".to_owned()),
            Event::ToolCallStarted {
                id: "t1".to_owned(),
                title: "Read".to_owned(),
                status: Some(ToolStatus::Pending),
                kind: Some(crowbot_protocol::ToolKind::Read),
                input: Some("notes.txt".to_owned()),
                effect: None,
            },
            Event::MessageText("hi".to_owned()),
            Event::TurnEnded(TurnEnd::Completed),
        ];
        for event in &events {
            store.append(id, event).unwrap();
        }
        assert_eq!(store.events(id).unwrap(), events);
    }

    #[test]
    fn events_stay_with_their_conversation() {
        let store = store();
        let first = conversation(&store);
        let second = conversation(&store);
        store
            .append(first, &Event::user_text("one".to_owned()))
            .unwrap();
        store
            .append(second, &Event::user_text("two".to_owned()))
            .unwrap();
        assert_eq!(
            store.events(second).unwrap(),
            vec![Event::user_text("two".to_owned())]
        );
    }

    #[test]
    fn a_missing_conversation_is_none() {
        assert!(store().conversation(42).unwrap().is_none());
        assert!(store().summary(42).unwrap().is_none());
    }

    #[test]
    fn a_conversation_takes_its_title_from_its_first_message_once() {
        let store = store();
        let id = conversation(&store);
        assert_eq!(store.summary(id).unwrap().unwrap().title, None);
        assert!(!store.set_title(id, "  \n ").unwrap());
        assert!(
            store
                .set_title(id, "\n  Plan the trip  \nwith details")
                .unwrap()
        );
        assert!(!store.set_title(id, "second message").unwrap());
        let summary = store.summary(id).unwrap().unwrap();
        assert_eq!(summary.title.as_deref(), Some("Plan the trip"));
        assert_eq!(
            store.conversation(id).unwrap().unwrap().title,
            summary.title
        );
    }

    #[test]
    fn a_long_first_message_gives_a_shortened_title() {
        let store = store();
        let id = conversation(&store);
        store.set_title(id, &"é".repeat(200)).unwrap();
        let title = store.summary(id).unwrap().unwrap().title.unwrap();
        assert_eq!(title.chars().count(), TITLE_LENGTH + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn a_bot_lists_its_conversations_with_the_most_recently_active_first() {
        let store = store();
        let owner = bot(&store);
        let first = owner.current_conversation_id;
        let second = store.new_conversation(owner.bot.id).unwrap().unwrap().id;
        let elsewhere = conversation(&store);
        store
            .connection
            .execute(
                "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (?1, 'user_text', ?2, ?3)",
                params![
                    first,
                    serde_json::to_string(&Event::user_text("later".to_owned())).unwrap(),
                    now_ms() + 60_000
                ],
            )
            .unwrap();
        let ids: Vec<i64> = store
            .summaries(owner.bot.id)
            .unwrap()
            .iter()
            .map(|summary| summary.id)
            .collect();
        assert_eq!(ids, vec![first, second]);
        assert!(!ids.contains(&elsewhere));
    }

    #[test]
    fn the_recorder_merges_adjacent_chunks_of_the_same_kind() {
        let store = store();
        let id = conversation(&store);
        let store = SharedStore::new(store);
        let mut recorder = Recorder::new(store.clone(), id);
        for event in [
            Event::MessageText("Hel".to_owned()),
            Event::MessageText("lo".to_owned()),
            Event::ThoughtText("hm".to_owned()),
            Event::MessageText(" again".to_owned()),
            Event::TurnEnded(TurnEnd::Completed),
        ] {
            recorder.record(event).unwrap();
        }
        assert_eq!(recorder.save_due(), None);
        assert_eq!(
            events(&store, id),
            vec![
                Event::MessageText("Hello".to_owned()),
                Event::ThoughtText("hm".to_owned()),
                Event::MessageText(" again".to_owned()),
                Event::TurnEnded(TurnEnd::Completed),
            ]
        );
    }

    #[test]
    fn a_save_writes_streamed_text_and_later_chunks_grow_the_same_row() {
        let store = store();
        let id = conversation(&store);
        let store = SharedStore::new(store);
        let mut recorder = Recorder::new(store.clone(), id);
        assert_eq!(recorder.save_due(), None);
        let before = Instant::now();
        recorder
            .record(Event::MessageText("partial".to_owned()))
            .unwrap();
        let due = recorder.save_due().unwrap();
        assert!(due >= before + SAVE_INTERVAL);
        assert!(due <= Instant::now() + SAVE_INTERVAL);
        assert!(SAVE_INTERVAL <= Duration::from_millis(500));
        assert!(events(&store, id).is_empty());

        recorder.save().unwrap();
        assert_eq!(recorder.save_due(), None);
        assert_eq!(
            events(&store, id),
            vec![Event::MessageText("partial".to_owned())]
        );

        recorder
            .record(Event::MessageText(" text".to_owned()))
            .unwrap();
        assert!(recorder.save_due().is_some());
        recorder.save().unwrap();
        assert_eq!(
            events(&store, id),
            vec![Event::MessageText("partial text".to_owned())]
        );
    }

    #[test]
    fn a_burst_of_text_is_saved_without_waiting_for_the_interval() {
        let store = store();
        let id = conversation(&store);
        let store = SharedStore::new(store);
        let mut recorder = Recorder::new(store.clone(), id);
        recorder
            .record(Event::MessageText("x".repeat(SAVE_BYTES)))
            .unwrap();
        assert_eq!(recorder.save_due(), None);
        assert_eq!(
            events(&store, id),
            vec![Event::MessageText("x".repeat(SAVE_BYTES))]
        );
    }

    #[test]
    fn a_long_stream_continues_in_a_new_row() {
        let store = store();
        let id = conversation(&store);
        let store = SharedStore::new(store);
        let mut recorder = Recorder::new(store.clone(), id);
        recorder
            .record(Event::MessageText("a".repeat(ROW_BYTES)))
            .unwrap();
        recorder
            .record(Event::MessageText("tail".to_owned()))
            .unwrap();
        recorder.close().unwrap();
        assert_eq!(
            events(&store, id),
            vec![
                Event::MessageText("a".repeat(ROW_BYTES)),
                Event::MessageText("tail".to_owned())
            ]
        );
    }

    fn version(store: &Store) -> i64 {
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn version_1_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (provider, cwd, provider_session_id, created_at_ms) VALUES ('claude', '/work', 'session-1', 7)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (1, 'user_text', ?1, 8)",
                params![serde_json::to_string(&Event::user_text("kept".to_owned())).unwrap()],
            )
            .unwrap();
        connection
    }

    #[test]
    fn a_version_1_database_migrates_and_keeps_its_rows() {
        let store = Store::from_connection(version_1_database()).unwrap();
        assert_eq!(version(&store), SCHEMA_VERSION);
        let conversation = store.conversation(1).unwrap().unwrap();
        let session = store.provider_session(1, "claude").unwrap().unwrap();
        assert_eq!(session.session_id, "session-1");
        assert_eq!(session.last_event_id, 1);
        assert_eq!(conversation.title.as_deref(), Some("kept"));
        assert_eq!(
            store.events(1).unwrap(),
            vec![Event::user_text("kept".to_owned())]
        );
        assert_eq!(store.queue(1).unwrap(), QueueState::default());
    }

    #[test]
    fn a_migration_that_fails_halfway_leaves_the_previous_version() {
        let connection = version_1_database();
        connection
            .execute_batch("CREATE TABLE queue (blocker INTEGER)")
            .unwrap();
        let store = Store { connection };
        assert!(store.migrate().is_err());
        assert_eq!(version(&store), 1);
        let paused_column: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'queue_paused'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(paused_column, 0);
        assert_eq!(
            store.events(1).unwrap(),
            vec![Event::user_text("kept".to_owned())]
        );
    }

    fn request(id: &str) -> Event {
        Event::ApprovalRequested {
            id: id.to_owned(),
            tool_call_id: format!("tool-{id}"),
            title: Some("Write notes.txt".to_owned()),
            target: Some("notes.txt".to_owned()),
            folder: Some("/work".to_owned()),
            cut: false,
        }
    }

    #[test]
    fn recovery_ends_a_running_turn_and_expires_its_waiting_approvals() {
        let store = store();
        let finished = conversation(&store);
        let running = conversation(&store);
        for event in [
            Event::user_text("done".to_owned()),
            Event::TurnEnded(TurnEnd::Completed),
        ] {
            store.append(finished, &event).unwrap();
        }
        for event in [
            Event::user_text("first".to_owned()),
            request("old"),
            Event::TurnEnded(TurnEnd::Completed),
            Event::user_text("second".to_owned()),
            Event::MessageText("partial".to_owned()),
            request("answered"),
            Event::ApprovalResolved {
                id: Some("answered".to_owned()),
                tool_call_id: "tool-answered".to_owned(),
                title: None,
                decision: crowbot_protocol::ApprovalDecision::Allowed,
                reason: None,
            },
            request("waiting"),
        ] {
            store.append(running, &event).unwrap();
        }

        assert_eq!(store.recover().unwrap(), 1);
        assert_eq!(store.events(finished).unwrap().len(), 2);
        let events = store.events(running).unwrap();
        assert_eq!(
            events[events.len() - 2..],
            [
                Event::ApprovalClosed {
                    id: "waiting".to_owned(),
                    tool_call_id: "tool-waiting".to_owned(),
                    title: Some("Write notes.txt".to_owned()),
                    reason: ApprovalClosure::Expired,
                },
                Event::TurnInterrupted {
                    cause: Interruption::DaemonRestarted
                },
            ]
        );

        assert_eq!(store.recover().unwrap(), 0);
        assert_eq!(store.events(running).unwrap(), events);
    }

    #[test]
    fn recovery_pauses_a_queue_that_still_holds_messages() {
        let store = store();
        let waiting = conversation(&store);
        let empty = conversation(&store);
        store.enqueue(waiting, "later").unwrap();
        store.recover().unwrap();
        assert!(store.queue(waiting).unwrap().paused);
        assert!(!store.queue(empty).unwrap().paused);
        assert!(store.events(waiting).unwrap().is_empty());
    }

    fn texts(store: &Store, id: i64) -> Vec<String> {
        store
            .queue(id)
            .unwrap()
            .messages
            .into_iter()
            .map(|message| message.text)
            .collect()
    }

    #[test]
    fn queued_messages_start_in_the_order_they_were_sent() {
        let store = store();
        let id = conversation(&store);
        let other = conversation(&store);
        store.enqueue(id, "one").unwrap();
        store.enqueue(other, "elsewhere").unwrap();
        store.enqueue(id, "two").unwrap();
        assert_eq!(texts(&store, id), ["one", "two"]);
        assert_eq!(
            store.start_queued(id, None).unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(
            store.start_queued(id, None).unwrap().as_deref(),
            Some("two")
        );
        assert_eq!(store.start_queued(id, None).unwrap(), None);
        assert_eq!(
            store.events(id).unwrap(),
            vec![
                Event::user_text("one".to_owned()),
                Event::user_text("two".to_owned())
            ]
        );
        assert_eq!(texts(&store, other), ["elsewhere"]);
    }

    #[test]
    fn a_queued_message_can_be_edited_deleted_and_moved_first() {
        let store = store();
        let id = conversation(&store);
        for text in ["one", "two", "three"] {
            store.enqueue(id, text).unwrap();
        }
        let ids: Vec<i64> = store
            .queue(id)
            .unwrap()
            .messages
            .iter()
            .map(|message| message.id)
            .collect();
        assert!(store.edit_queued(id, ids[1], "two, edited").unwrap());
        assert!(store.delete_queued(id, ids[0]).unwrap());
        assert!(store.move_queued_first(id, ids[2]).unwrap());
        assert_eq!(texts(&store, id), ["three", "two, edited"]);
        store.enqueue(id, "four").unwrap();
        assert_eq!(texts(&store, id), ["three", "two, edited", "four"]);

        assert!(!store.edit_queued(id, ids[0], "gone").unwrap());
        assert!(!store.delete_queued(id, ids[0]).unwrap());
        assert!(!store.move_queued_first(id, ids[0]).unwrap());
        assert!(!store.edit_queued(id + 1, ids[1], "other").unwrap());
    }

    #[test]
    fn a_queue_that_empties_is_no_longer_paused() {
        let store = store();
        let id = conversation(&store);
        store.enqueue(id, "one").unwrap();
        store.set_queue_paused(id, true).unwrap();
        assert!(store.queue(id).unwrap().paused);
        let queued = store.queue(id).unwrap().messages[0].id;
        store.delete_queued(id, queued).unwrap();
        store.enqueue(id, "two").unwrap();
        assert!(!store.queue(id).unwrap().paused);
    }

    fn page_texts(page: &EventsPage) -> Vec<String> {
        page.events
            .iter()
            .map(|timed| match &timed.event {
                Event::UserText(message) => message.text.clone(),
                other => panic!("unexpected event {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_page_holds_the_newest_events_in_order_and_tells_where_older_ones_start() {
        let store = store();
        let id = conversation(&store);
        let other = conversation(&store);
        let mut rows = Vec::new();
        for index in 0..7 {
            store
                .append(other, &Event::user_text(format!("other {index}")))
                .unwrap();
            rows.push(
                store
                    .append(id, &Event::user_text(format!("m{index}")))
                    .unwrap(),
            );
        }

        let newest = store.events_page(id, None, 3).unwrap();
        assert_eq!(page_texts(&newest), ["m4", "m5", "m6"]);
        assert_eq!(newest.older, Some(rows[4]));
        assert!(newest.events.iter().all(|timed| timed.at_ms > 0));

        let middle = store.events_page(id, newest.older, 3).unwrap();
        assert_eq!(page_texts(&middle), ["m1", "m2", "m3"]);
        assert_eq!(middle.older, Some(rows[1]));

        let oldest = store.events_page(id, middle.older, 3).unwrap();
        assert_eq!(page_texts(&oldest), ["m0"]);
        assert_eq!(oldest.older, None);

        let exact = store.events_page(id, None, 7).unwrap();
        assert_eq!(exact.events.len(), 7);
        assert_eq!(exact.older, None);
        let nothing = store.events_page(id, Some(rows[0]), 3).unwrap();
        assert!(nothing.events.is_empty());
        assert_eq!(nothing.older, None);
        assert!(store.events_page(42, None, 3).unwrap().events.is_empty());
    }

    #[test]
    fn a_page_gives_each_event_the_time_it_was_stored() {
        let store = store();
        let id = conversation(&store);
        for (payload, at_ms) in [("first", 1_000), ("second", 2_500)] {
            store
                .connection
                .execute(
                    "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (?1, 'user_text', ?2, ?3)",
                    params![
                        id,
                        serde_json::to_string(&Event::user_text(payload.to_owned())).unwrap(),
                        at_ms
                    ],
                )
                .unwrap();
        }
        let page = store.events_page(id, None, 10).unwrap();
        assert_eq!(
            page.events,
            vec![
                TimedEvent {
                    id: 1,
                    at_ms: 1_000,
                    event: Event::user_text("first".to_owned()),
                },
                TimedEvent {
                    id: 2,
                    at_ms: 2_500,
                    event: Event::user_text("second".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn a_draft_is_stored_with_its_conversation() {
        let store = store();
        let id = conversation(&store);
        let other = conversation(&store);
        assert_eq!(store.draft(id).unwrap(), "");
        store.set_draft(id, "half a thought\nand a line").unwrap();
        assert_eq!(store.draft(id).unwrap(), "half a thought\nand a line");
        assert_eq!(store.draft(other).unwrap(), "");
        store.set_draft(id, "").unwrap();
        assert_eq!(store.draft(id).unwrap(), "");
        store.set_draft(42, "nowhere").unwrap();
        assert_eq!(store.draft(42).unwrap(), "");
    }

    #[test]
    fn a_version_7_database_gives_each_bot_a_mascot_and_keeps_its_rows() {
        let connection = Connection::open_in_memory().unwrap();
        for schema in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
        ] {
            connection.execute_batch(schema).unwrap();
        }
        connection.pragma_update(None, "user_version", 7).unwrap();
        connection
            .execute_batch(
                r##"INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
                    VALUES ('Atlas', '#3b82f6', 'A', 'claude', '/work', 5),
                        ('Owl', '#10b981', 'O', 'claude', '/work', 6),
                        ('Fox', '#e8483f', 'F', 'claude', '/work', 7);"##,
            )
            .unwrap();
        let store = Store::from_connection(connection).unwrap();
        assert_eq!(version(&store), SCHEMA_VERSION);
        let bots = store.bots().unwrap();
        let faces: Vec<_> = bots.iter().map(|row| row.bot.avatar.shape).collect();
        assert_eq!(
            faces,
            [
                crowbot_protocol::mascot_at(0),
                crowbot_protocol::mascot_at(1),
                crowbot_protocol::mascot_at(2)
            ]
        );
        assert_eq!(bots[1].bot.avatar.color, "#10b981");
        // The picture a bot had stays in the database.
        let glyph: String = store
            .connection
            .query_row("SELECT avatar_glyph FROM bots WHERE id = 2", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(glyph, "O");
    }

    #[test]
    fn a_version_4_database_gains_drafts_and_keeps_its_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(SCHEMA_V2).unwrap();
        connection.execute_batch(SCHEMA_V3).unwrap();
        connection.execute_batch(SCHEMA_V4).unwrap();
        connection.pragma_update(None, "user_version", 4).unwrap();
        connection
            .execute_batch(
                r#"INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
                    VALUES ('Atlas', '#3b82f6', 'A', 'claude', '/work', 5);
                 INSERT INTO conversations (bot_id, provider, cwd, created_at_ms)
                    VALUES (1, 'claude', '/work', 6);
                 UPDATE bots SET current_conversation_id = 1;
                 INSERT INTO preferences (key, value) VALUES ('appearance', 'dark');
                 INSERT INTO queue (conversation_id, text, created_at_ms) VALUES (1, 'later', 9);
                 INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES
                    (1, 'user_text', '{"type":"user_text","data":"kept"}', 7),
                    (1, 'tool_call_started', '{"type":"tool_call_started","data":{"id":"t1","title":"Read","status":"pending"}}', 8),
                    (1, 'tool_call_updated', '{"type":"tool_call_updated","data":{"id":"t1","title":null,"status":"completed"}}', 9),
                    (1, 'approval_requested', '{"type":"approval_requested","data":{"id":"a1","tool_call_id":"t1","title":"Write"}}', 10),
                    (1, 'turn_failed', '{"type":"turn_failed","data":{"message":"gone"}}', 11);"#,
            )
            .unwrap();
        let store = Store::from_connection(connection).unwrap();
        assert_eq!(version(&store), SCHEMA_VERSION);
        assert_eq!(store.bots().unwrap().len(), 1);
        assert_eq!(texts(&store, 1), ["later"]);
        assert_eq!(
            store.preferences().unwrap().appearance,
            crowbot_protocol::Appearance::Dark
        );
        assert_eq!(store.draft(1).unwrap(), "");
        store.set_draft(1, "new").unwrap();
        assert_eq!(store.draft(1).unwrap(), "new");

        let page = store.events_page(1, None, 200).unwrap();
        assert_eq!(page.older, None);
        let times: Vec<i64> = page.events.iter().map(|timed| timed.at_ms).collect();
        assert_eq!(times, [7, 8, 9, 10, 11]);
        let events: Vec<Event> = page.events.into_iter().map(|timed| timed.event).collect();
        assert_eq!(
            events,
            vec![
                Event::user_text("kept".to_owned()),
                Event::ToolCallStarted {
                    id: "t1".to_owned(),
                    title: "Read".to_owned(),
                    status: Some(ToolStatus::Pending),
                    kind: None,
                    input: None,
                    effect: None,
                },
                Event::ToolCallUpdated {
                    id: "t1".to_owned(),
                    title: None,
                    status: Some(ToolStatus::Completed),
                    kind: None,
                    input: None,
                    output: None,
                },
                Event::ApprovalRequested {
                    id: "a1".to_owned(),
                    tool_call_id: "t1".to_owned(),
                    title: Some("Write".to_owned()),
                    target: None,
                    folder: None,
                    cut: false,
                },
                Event::TurnFailed {
                    message: "gone".to_owned(),
                    cause: None,
                    provider: None,
                },
            ]
        );
    }

    #[test]
    fn a_database_from_a_newer_daemon_is_refused() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(
            Store::from_connection(connection),
            Err(StoreError::NewerSchema(99))
        ));
    }

    #[test]
    fn opening_twice_keeps_the_data() {
        let dir = crowbot_protocol::scratch::Scratch::new("crowbot-store-test");
        let path = dir.join("crowbot.db");
        let id = {
            let store = Store::open(&path).unwrap();
            let id = conversation(&store);
            store
                .append(id, &Event::user_text("kept".to_owned()))
                .unwrap();
            id
        };
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.events(id).unwrap(),
            vec![Event::user_text("kept".to_owned())]
        );
    }
}
