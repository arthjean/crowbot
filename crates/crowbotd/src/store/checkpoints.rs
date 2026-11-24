//! The records of a bot's folder (EP-008): each one names the tree of the
//! folder it saw, and each tree is stored once, by the SHA-256 of its body,
//! so that two records of an unchanged folder share every row. The contents
//! of files are not here: they are files of the data folder, named by their
//! hash (`checkpoints/objects.rs`).

use crowbot_protocol::{CheckpointMoment, CheckpointSummary};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError};

/// A record to store.
pub struct NewCheckpoint<'a> {
    pub conversation_id: i64,
    pub bot_id: i64,
    /// The `UserText` event of the turn the record belongs to: the turn it
    /// was taken around, or the turn a restore went back to.
    pub turn_event_id: Option<i64>,
    /// The event the record is tied to: that `UserText`, the end of the
    /// turn, or a `CheckpointRestored`.
    pub event_id: Option<i64>,
    pub moment: CheckpointMoment,
    /// The folder, as the daemon reads it with its links resolved.
    pub folder: &'a str,
    /// The hash of the folder's tree.
    pub tree: &'a str,
    pub files: u64,
    pub bytes: u64,
    pub stored_bytes: u64,
    pub taken_at_ms: i64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRow {
    pub id: i64,
    pub conversation_id: i64,
    pub bot_id: i64,
    pub turn_event_id: Option<i64>,
    pub event_id: Option<i64>,
    pub folder: String,
    pub tree: String,
    pub summary: CheckpointSummary,
}

const COLUMNS: &str = "id, conversation_id, bot_id, turn_event_id, event_id, moment, folder, tree,
    files, bytes, stored_bytes, taken_at_ms, duration_ms";

fn moment_name(moment: CheckpointMoment) -> &'static str {
    match moment {
        CheckpointMoment::BeforeTurn => "before_turn",
        CheckpointMoment::AfterTurn => "after_turn",
        CheckpointMoment::BeforeRestore => "before_restore",
        CheckpointMoment::AfterRestore => "after_restore",
        CheckpointMoment::Other => "other",
    }
}

fn moment_of(name: &str) -> CheckpointMoment {
    match name {
        "before_turn" => CheckpointMoment::BeforeTurn,
        "after_turn" => CheckpointMoment::AfterTurn,
        "before_restore" => CheckpointMoment::BeforeRestore,
        "after_restore" => CheckpointMoment::AfterRestore,
        _ => CheckpointMoment::Other,
    }
}

fn figure(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<CheckpointRow> {
    let unsigned = |index: usize| -> rusqlite::Result<u64> {
        Ok(u64::try_from(row.get::<_, i64>(index)?).unwrap_or(0))
    };
    Ok(CheckpointRow {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        bot_id: row.get(2)?,
        turn_event_id: row.get(3)?,
        event_id: row.get(4)?,
        folder: row.get(6)?,
        tree: row.get(7)?,
        summary: CheckpointSummary {
            moment: moment_of(&row.get::<_, String>(5)?),
            files: unsigned(8)?,
            bytes: unsigned(9)?,
            stored_bytes: unsigned(10)?,
            taken_at_ms: row.get(11)?,
            duration_ms: unsigned(12)?,
        },
    })
}

impl Store {
    /// Stores a record with the trees it names, together: a record is never
    /// stored without its trees. A tree already stored is kept as it is.
    pub fn record_checkpoint(
        &self,
        new: &NewCheckpoint<'_>,
        trees: &[(String, String)],
    ) -> Result<i64, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT OR IGNORE INTO checkpoint_trees (hash, body) VALUES (?1, ?2)",
            )?;
            for (hash, body) in trees {
                insert.execute(params![hash, body])?;
            }
        }
        transaction.execute(
            "INSERT INTO checkpoints (conversation_id, bot_id, turn_event_id, event_id, moment,
                folder, tree, files, bytes, stored_bytes, taken_at_ms, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                new.conversation_id,
                new.bot_id,
                new.turn_event_id,
                new.event_id,
                moment_name(new.moment),
                new.folder,
                new.tree,
                figure(new.files),
                figure(new.bytes),
                figure(new.stored_bytes),
                new.taken_at_ms,
                figure(new.duration_ms),
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(id)
    }

    /// Ties a record taken before its event existed to that event.
    pub fn tie_checkpoint(&self, checkpoint_id: i64, event_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE checkpoints SET event_id = ?2 WHERE id = ?1",
            params![checkpoint_id, event_id],
        )?;
        Ok(())
    }

    /// The record of `moment` taken around the turn that the `UserText`
    /// event `turn_event_id` of the conversation started.
    pub fn turn_checkpoint(
        &self,
        conversation_id: i64,
        turn_event_id: i64,
        moment: CheckpointMoment,
    ) -> Result<Option<CheckpointRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM checkpoints
                     WHERE conversation_id = ?1 AND turn_event_id = ?2 AND moment = ?3
                     ORDER BY id DESC LIMIT 1"
                ),
                params![conversation_id, turn_event_id, moment_name(moment)],
                read_row,
            )
            .optional()?;
        Ok(row)
    }

    /// The last record taken of a folder, by any bot and in any conversation.
    pub fn last_checkpoint(&self, folder: &str) -> Result<Option<CheckpointRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM checkpoints WHERE folder = ?1 ORDER BY id DESC LIMIT 1"
                ),
                params![folder],
                read_row,
            )
            .optional()?;
        Ok(row)
    }

    /// The body of a tree.
    pub fn checkpoint_tree(&self, hash: &str) -> Result<Option<String>, StoreError> {
        let body = self
            .connection
            .query_row(
                "SELECT body FROM checkpoint_trees WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(body)
    }

    /// The trees the records name, each once.
    pub fn checkpoint_roots(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT tree FROM checkpoints")?;
        let roots = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(roots)
    }

    /// The hashes of every stored tree.
    pub fn checkpoint_tree_hashes(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT hash FROM checkpoint_trees")?;
        let hashes = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(hashes)
    }

    /// Removes trees that no record reaches any longer.
    pub fn delete_checkpoint_trees(&self, hashes: &[String]) -> Result<(), StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut delete =
                transaction.prepare_cached("DELETE FROM checkpoint_trees WHERE hash = ?1")?;
            for hash in hashes {
                delete.execute(params![hash])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Whether the event is the `UserText` that started a turn of the
    /// conversation.
    pub fn is_turn_start(&self, conversation_id: i64, event_id: i64) -> Result<bool, StoreError> {
        let found = self
            .connection
            .query_row(
                "SELECT 1 FROM events WHERE id = ?1 AND conversation_id = ?2 AND kind = 'user_text'",
                params![event_id, conversation_id],
                |_| Ok(()),
            )
            .optional()?;
        Ok(found.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewBot;
    use crowbot_protocol::Event;

    fn store_with_conversation() -> (Store, i64, i64) {
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
                workspace: "/tmp/atlas".to_owned(),
            })
            .unwrap();
        (store, bot.bot.id, bot.current_conversation_id)
    }

    fn record(store: &Store, conversation_id: i64, bot_id: i64, tree: &str) -> i64 {
        store
            .record_checkpoint(
                &NewCheckpoint {
                    conversation_id,
                    bot_id,
                    turn_event_id: Some(1),
                    event_id: Some(1),
                    moment: CheckpointMoment::BeforeTurn,
                    folder: "/tmp/atlas",
                    tree,
                    files: 2,
                    bytes: 10,
                    stored_bytes: 10,
                    taken_at_ms: 1_000,
                    duration_ms: 7,
                },
                &[(tree.to_owned(), "[]".to_owned())],
            )
            .unwrap()
    }

    #[test]
    fn a_record_keeps_its_figures_and_the_time_it_took() {
        let (store, bot_id, conversation_id) = store_with_conversation();
        let id = record(&store, conversation_id, bot_id, "aa");
        let row = store
            .turn_checkpoint(conversation_id, 1, CheckpointMoment::BeforeTurn)
            .unwrap()
            .unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.summary.duration_ms, 7);
        assert_eq!(row.summary.files, 2);
        assert_eq!(store.last_checkpoint("/tmp/atlas").unwrap().unwrap().id, id);
        assert_eq!(store.checkpoint_tree("aa").unwrap().as_deref(), Some("[]"));
    }

    #[test]
    fn two_records_of_one_tree_store_it_once() {
        let (store, bot_id, conversation_id) = store_with_conversation();
        record(&store, conversation_id, bot_id, "aa");
        record(&store, conversation_id, bot_id, "aa");
        assert_eq!(
            store.checkpoint_tree_hashes().unwrap(),
            vec!["aa".to_owned()]
        );
        assert_eq!(store.checkpoint_roots().unwrap(), vec!["aa".to_owned()]);
    }

    #[test]
    fn a_turn_starts_at_a_user_text_of_its_conversation() {
        let (store, _, conversation_id) = store_with_conversation();
        let said = store
            .append(conversation_id, &Event::MessageText("hi".to_owned()))
            .unwrap();
        let asked = store
            .append(conversation_id, &Event::user_text("go"))
            .unwrap();
        assert!(store.is_turn_start(conversation_id, asked).unwrap());
        assert!(!store.is_turn_start(conversation_id, said).unwrap());
        assert!(!store.is_turn_start(conversation_id + 1, asked).unwrap());
    }
}
