use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{ConversationSummary, Event};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA_VERSION: i64 = 1;
const TITLE_LENGTH: usize = 80;

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

const SUMMARIES: &str = "
SELECT
    c.id,
    c.cwd,
    c.created_at_ms,
    (SELECT payload FROM events
        WHERE conversation_id = c.id AND kind = 'user_text' ORDER BY id LIMIT 1),
    COALESCE((SELECT MAX(created_at_ms) FROM events WHERE conversation_id = c.id), c.created_at_ms)
FROM conversations c
WHERE ?1 IS NULL OR c.id = ?1
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
    pub provider: String,
    pub command: Option<String>,
    pub cwd: PathBuf,
    pub provider_session_id: Option<String>,
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
        Ok(())
    }

    pub fn create_conversation(
        &self,
        provider: &str,
        command: Option<&str>,
        cwd: &Path,
    ) -> Result<Conversation, StoreError> {
        self.connection.execute(
            "INSERT INTO conversations (provider, command, cwd, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![provider, command, cwd.to_string_lossy(), now_ms()],
        )?;
        Ok(Conversation {
            id: self.connection.last_insert_rowid(),
            provider: provider.to_owned(),
            command: command.map(str::to_owned),
            cwd: cwd.to_path_buf(),
            provider_session_id: None,
        })
    }

    pub fn conversation(&self, id: i64) -> Result<Option<Conversation>, StoreError> {
        let conversation = self
            .connection
            .query_row(
                "SELECT id, provider, command, cwd, provider_session_id FROM conversations WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Conversation {
                        id: row.get(0)?,
                        provider: row.get(1)?,
                        command: row.get(2)?,
                        cwd: PathBuf::from(row.get::<_, String>(3)?),
                        provider_session_id: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(conversation)
    }

    pub fn summaries(&self) -> Result<Vec<ConversationSummary>, StoreError> {
        self.query_summaries(None)
    }

    pub fn summary(&self, id: i64) -> Result<Option<ConversationSummary>, StoreError> {
        Ok(self.query_summaries(Some(id))?.pop())
    }

    fn query_summaries(&self, id: Option<i64>) -> Result<Vec<ConversationSummary>, StoreError> {
        let mut statement = self.connection.prepare(SUMMARIES)?;
        let summaries = statement
            .query_map(params![id], |row| {
                Ok(ConversationSummary {
                    id: row.get(0)?,
                    cwd: row.get(1)?,
                    created_at_ms: row.get(2)?,
                    title: row
                        .get::<_, Option<String>>(3)?
                        .as_deref()
                        .and_then(title_from),
                    last_activity_ms: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(summaries)
    }

    pub fn set_provider_session(
        &self,
        conversation_id: i64,
        provider_session_id: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE conversations SET provider_session_id = ?2 WHERE id = ?1",
            params![conversation_id, provider_session_id],
        )?;
        Ok(())
    }

    pub fn append(&self, conversation_id: i64, event: &Event) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![
                conversation_id,
                kind(event),
                serde_json::to_string(event)?,
                now_ms()
            ],
        )?;
        Ok(())
    }

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

fn kind(event: &Event) -> &'static str {
    match event {
        Event::UserText(_) => "user_text",
        Event::MessageText(_) => "message_text",
        Event::ThoughtText(_) => "thought_text",
        Event::ToolCallStarted { .. } => "tool_call_started",
        Event::ToolCallUpdated { .. } => "tool_call_updated",
        Event::ApprovalRequested { .. } => "approval_requested",
        Event::ApprovalResolved { .. } => "approval_resolved",
        Event::TurnEnded(_) => "turn_ended",
        Event::TurnFailed { .. } => "turn_failed",
    }
}

fn title_from(payload: &str) -> Option<String> {
    let Ok(Event::UserText(text)) = serde_json::from_str(payload) else {
        return None;
    };
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
}

pub struct Recorder {
    store: SharedStore,
    conversation_id: i64,
    pending: Option<Event>,
}

impl Recorder {
    pub fn new(store: SharedStore, conversation_id: i64) -> Self {
        Self {
            store,
            conversation_id,
            pending: None,
        }
    }

    pub fn record(&mut self, event: Event) -> Result<(), StoreError> {
        match (&mut self.pending, event) {
            (Some(Event::MessageText(buffer)), Event::MessageText(text))
            | (Some(Event::ThoughtText(buffer)), Event::ThoughtText(text)) => {
                buffer.push_str(&text);
                Ok(())
            }
            (_, event @ (Event::MessageText(_) | Event::ThoughtText(_))) => {
                self.flush()?;
                self.pending = Some(event);
                Ok(())
            }
            (_, event) => {
                self.flush()?;
                self.append(&event)
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), StoreError> {
        match self.pending.take() {
            Some(event) => self.append(&event),
            None => Ok(()),
        }
    }

    fn append(&self, event: &Event) -> Result<(), StoreError> {
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

    fn conversation(store: &Store) -> i64 {
        store
            .create_conversation("claude", None, Path::new("/work"))
            .unwrap()
            .id
    }

    fn events(store: &SharedStore, id: i64) -> Vec<Event> {
        store.with(|store| store.events(id)).unwrap()
    }

    #[test]
    fn events_come_back_in_the_order_they_were_appended() {
        let store = store();
        let id = conversation(&store);
        let events = vec![
            Event::UserText("hello".to_owned()),
            Event::ToolCallStarted {
                id: "t1".to_owned(),
                title: "Read".to_owned(),
                status: Some(ToolStatus::Pending),
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
            .append(first, &Event::UserText("one".to_owned()))
            .unwrap();
        store
            .append(second, &Event::UserText("two".to_owned()))
            .unwrap();
        assert_eq!(
            store.events(second).unwrap(),
            vec![Event::UserText("two".to_owned())]
        );
    }

    #[test]
    fn a_missing_conversation_is_none() {
        assert!(store().conversation(42).unwrap().is_none());
        assert!(store().summary(42).unwrap().is_none());
    }

    #[test]
    fn a_conversation_keeps_its_provider_command_and_folder() {
        let store = store();
        let created = store
            .create_conversation("custom", Some("my-agent --acp"), Path::new("/work"))
            .unwrap();
        let loaded = store.conversation(created.id).unwrap().unwrap();
        assert_eq!(loaded.provider, "custom");
        assert_eq!(loaded.command.as_deref(), Some("my-agent --acp"));
        assert_eq!(loaded.cwd, PathBuf::from("/work"));
        assert_eq!(loaded.provider_session_id, None);
    }

    #[test]
    fn a_summary_takes_its_title_from_the_first_user_message() {
        let store = store();
        let id = conversation(&store);
        assert_eq!(store.summary(id).unwrap().unwrap().title, None);
        store
            .append(
                id,
                &Event::UserText("\n  Plan the trip  \nwith details".to_owned()),
            )
            .unwrap();
        store
            .append(id, &Event::UserText("second message".to_owned()))
            .unwrap();
        let summary = store.summary(id).unwrap().unwrap();
        assert_eq!(summary.title.as_deref(), Some("Plan the trip"));
        assert_eq!(summary.cwd, "/work");
    }

    #[test]
    fn a_long_first_message_gives_a_shortened_title() {
        let store = store();
        let id = conversation(&store);
        store.append(id, &Event::UserText("é".repeat(200))).unwrap();
        let title = store.summary(id).unwrap().unwrap().title.unwrap();
        assert_eq!(title.chars().count(), TITLE_LENGTH + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn summaries_list_the_most_recently_active_conversation_first() {
        let store = store();
        let first = conversation(&store);
        let second = conversation(&store);
        store
            .connection
            .execute(
                "INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES (?1, 'user_text', ?2, ?3)",
                params![
                    first,
                    serde_json::to_string(&Event::UserText("later".to_owned())).unwrap(),
                    now_ms() + 60_000
                ],
            )
            .unwrap();
        let ids: Vec<i64> = store
            .summaries()
            .unwrap()
            .iter()
            .map(|summary| summary.id)
            .collect();
        assert_eq!(ids, vec![first, second]);
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
    fn the_recorder_keeps_pending_text_until_flush() {
        let store = store();
        let id = conversation(&store);
        let store = SharedStore::new(store);
        let mut recorder = Recorder::new(store.clone(), id);
        recorder
            .record(Event::MessageText("partial".to_owned()))
            .unwrap();
        assert!(events(&store, id).is_empty());
        recorder.flush().unwrap();
        assert_eq!(
            events(&store, id),
            vec![Event::MessageText("partial".to_owned())]
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
        let path = std::env::temp_dir().join(format!("crowbot-store-test-{}.db", now_ms()));
        let id = {
            let store = Store::open(&path).unwrap();
            let id = conversation(&store);
            store
                .append(id, &Event::UserText("kept".to_owned()))
                .unwrap();
            id
        };
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.events(id).unwrap(),
            vec![Event::UserText("kept".to_owned())]
        );
        drop(reopened);
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
        }
    }
}
