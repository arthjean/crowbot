use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::event::Event;

const SCHEMA_VERSION: i64 = 1;

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
                event.kind(),
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

pub struct Recorder {
    store: Store,
    conversation_id: i64,
    pending: Option<Event>,
}

impl Recorder {
    pub fn new(store: Store, conversation_id: i64) -> Self {
        Self {
            store,
            conversation_id,
            pending: None,
        }
    }

    pub fn record(&mut self, event: Event) -> Result<(), StoreError> {
        match event {
            Event::SessionStarted {
                provider_session_id,
            } => self
                .store
                .set_provider_session(self.conversation_id, &provider_session_id),
            Event::MessageText(_) | Event::ThoughtText(_) => self.buffer(event),
            other => {
                self.flush()?;
                self.store.append(self.conversation_id, &other)
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), StoreError> {
        match self.pending.take() {
            Some(event) => self.store.append(self.conversation_id, &event),
            None => Ok(()),
        }
    }

    fn buffer(&mut self, event: Event) -> Result<(), StoreError> {
        match (&mut self.pending, &event) {
            (Some(Event::MessageText(buffer)), Event::MessageText(text))
            | (Some(Event::ThoughtText(buffer)), Event::ThoughtText(text)) => {
                buffer.push_str(text);
                Ok(())
            }
            _ => {
                self.flush()?;
                self.pending = Some(event);
                Ok(())
            }
        }
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
    use super::*;
    use crate::event::{ToolStatus, TurnEnd};

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn conversation(store: &Store) -> i64 {
        store
            .create_conversation("claude", None, Path::new("/work"))
            .unwrap()
            .id
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
    fn the_recorder_merges_adjacent_chunks_of_the_same_kind() {
        let store = store();
        let id = conversation(&store);
        let mut recorder = Recorder::new(store, id);
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
            recorder.store.events(id).unwrap(),
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
        let mut recorder = Recorder::new(store, id);
        recorder
            .record(Event::MessageText("partial".to_owned()))
            .unwrap();
        assert!(recorder.store.events(id).unwrap().is_empty());
        recorder.flush().unwrap();
        assert_eq!(
            recorder.store.events(id).unwrap(),
            vec![Event::MessageText("partial".to_owned())]
        );
    }

    #[test]
    fn the_recorder_saves_the_provider_session_on_the_conversation() {
        let store = store();
        let id = conversation(&store);
        let mut recorder = Recorder::new(store, id);
        recorder
            .record(Event::SessionStarted {
                provider_session_id: "session-1".to_owned(),
            })
            .unwrap();
        let loaded = recorder.store.conversation(id).unwrap().unwrap();
        assert_eq!(loaded.provider_session_id.as_deref(), Some("session-1"));
        assert!(recorder.store.events(id).unwrap().is_empty());
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
