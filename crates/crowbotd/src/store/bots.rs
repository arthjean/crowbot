//! Bots in the database: their records, their conversations, and their read marks.

use crowbot_protocol::{Avatar, Bot, BotPatch, ConversationSummary, Event, PermissionLevel};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, now_ms, title_of};

const BOT_COLUMNS: &str =
    "id, name, title, instructions, avatar_color, avatar_glyph, provider, model,
    permission_level, workspace, pinned, hidden, created_at_ms, current_conversation_id,
    session_revision";

/// The bot that takes the conversations stored before bots existed.
const ADOPTER_NAME: &str = "Crow";
const ADOPTER_COLOR: &str = "#3b82f6";

pub struct BotRow {
    pub bot: Bot,
    pub current_conversation_id: i64,
    /// Grows each time the instructions, the model, or the folder change. A
    /// provider session started with an older revision is not resumed.
    pub session_revision: i64,
}

pub struct NewBot {
    pub name: String,
    pub title: String,
    pub instructions: String,
    pub avatar: Avatar,
    pub provider: String,
    pub model: Option<String>,
    pub permission_level: PermissionLevel,
    pub workspace: String,
}

/// What the stored events say about a bot, over all its conversations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// The turns that ended after the read mark of their conversation.
    pub unread: u32,
    /// A conversation whose last turn failed or was interrupted, after its read mark.
    pub failed: bool,
    pub last_activity_ms: i64,
}

fn level_name(level: PermissionLevel) -> &'static str {
    match level {
        PermissionLevel::AskFirst => "ask_first",
        PermissionLevel::FullAccess => "full_access",
    }
}

/// A level this daemon does not know asks first.
fn level_from(name: &str) -> PermissionLevel {
    match name {
        "full_access" => PermissionLevel::FullAccess,
        _ => PermissionLevel::AskFirst,
    }
}

fn bot_row(row: &Row<'_>) -> rusqlite::Result<BotRow> {
    Ok(BotRow {
        bot: Bot {
            id: row.get(0)?,
            name: row.get(1)?,
            title: row.get(2)?,
            instructions: row.get(3)?,
            avatar: Avatar {
                color: row.get(4)?,
                glyph: row.get(5)?,
            },
            provider: row.get(6)?,
            model: row.get(7)?,
            permission_level: level_from(&row.get::<_, String>(8)?),
            workspace: row.get(9)?,
            pinned: row.get(10)?,
            hidden: row.get(11)?,
            created_at_ms: row.get(12)?,
        },
        current_conversation_id: row.get::<_, Option<i64>>(13)?.unwrap_or_default(),
        session_revision: row.get(14)?,
    })
}

impl Store {
    /// Stores a bot with its first conversation, which is its current one.
    pub fn create_bot(&self, new: &NewBot) -> Result<BotRow, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO bots (name, title, instructions, avatar_color, avatar_glyph, provider,
                model, permission_level, workspace, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new.name,
                new.title,
                new.instructions,
                new.avatar.color,
                new.avatar.glyph,
                new.provider,
                new.model,
                level_name(new.permission_level),
                new.workspace,
                now_ms()
            ],
        )?;
        let id = transaction.last_insert_rowid();
        self.insert_conversation(id)?;
        let created = self.bot(id)?;
        transaction.commit()?;
        created.ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn bot(&self, id: i64) -> Result<Option<BotRow>, StoreError> {
        let bot = self
            .connection
            .query_row(
                &format!("SELECT {BOT_COLUMNS} FROM bots WHERE id = ?1"),
                params![id],
                bot_row,
            )
            .optional()?;
        Ok(bot)
    }

    pub fn bots(&self) -> Result<Vec<BotRow>, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT {BOT_COLUMNS} FROM bots ORDER BY id"))?;
        let bots = statement
            .query_map([], bot_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(bots)
    }

    /// Applies the fields that the patch holds. `None` means the bot does not exist.
    pub fn update_bot(&self, id: i64, patch: &BotPatch) -> Result<Option<BotRow>, StoreError> {
        let Some(BotRow {
            mut bot,
            session_revision,
            ..
        }) = self.bot(id)?
        else {
            return Ok(None);
        };
        let session_settings = |bot: &Bot| {
            (
                bot.instructions.clone(),
                bot.model.clone(),
                bot.workspace.clone(),
            )
        };
        let before = session_settings(&bot);
        let BotPatch {
            name,
            title,
            instructions,
            avatar,
            model,
            permission_level,
            workspace,
            pinned,
            hidden,
        } = patch.clone();
        bot.name = name.unwrap_or(bot.name);
        bot.title = title.unwrap_or(bot.title);
        bot.instructions = instructions.unwrap_or(bot.instructions);
        bot.avatar = avatar.unwrap_or(bot.avatar);
        bot.model = model.unwrap_or(bot.model);
        bot.permission_level = permission_level.unwrap_or(bot.permission_level);
        bot.workspace = workspace.unwrap_or(bot.workspace);
        bot.pinned = pinned.unwrap_or(bot.pinned);
        bot.hidden = hidden.unwrap_or(bot.hidden);
        let revision = session_revision + i64::from(session_settings(&bot) != before);
        self.connection.execute(
            "UPDATE bots SET name = ?2, title = ?3, instructions = ?4, avatar_color = ?5,
                avatar_glyph = ?6, model = ?7, permission_level = ?8, workspace = ?9,
                pinned = ?10, hidden = ?11, session_revision = ?12
             WHERE id = ?1",
            params![
                id,
                bot.name,
                bot.title,
                bot.instructions,
                bot.avatar.color,
                bot.avatar.glyph,
                bot.model,
                level_name(bot.permission_level),
                bot.workspace,
                bot.pinned,
                bot.hidden,
                revision
            ],
        )?;
        self.bot(id)
    }

    /// Removes a bot with its conversations, their events, and their queues.
    pub fn delete_bot(&self, id: i64) -> Result<bool, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        for owned in ["queue", "events"] {
            transaction.execute(
                &format!(
                    "DELETE FROM {owned} WHERE conversation_id IN
                        (SELECT id FROM conversations WHERE bot_id = ?1)"
                ),
                params![id],
            )?;
        }
        transaction.execute("DELETE FROM conversations WHERE bot_id = ?1", params![id])?;
        let deleted = transaction.execute("DELETE FROM bots WHERE id = ?1", params![id])?;
        transaction.commit()?;
        Ok(deleted > 0)
    }

    pub fn conversation_ids(&self, bot_id: i64) -> Result<Vec<i64>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM conversations WHERE bot_id = ?1 ORDER BY id")?;
        let ids = statement
            .query_map(params![bot_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// Starts a conversation and makes it the bot's current one. `None` means
    /// the bot does not exist.
    pub fn new_conversation(&self, bot_id: i64) -> Result<Option<ConversationSummary>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let Some(id) = self.insert_conversation(bot_id)? else {
            return Ok(None);
        };
        let summary = self.summary(id)?;
        transaction.commit()?;
        Ok(summary)
    }

    /// Makes a conversation the current one of its bot: the one a client
    /// shows when the bot is opened. `false` means it already was.
    pub fn make_current(&self, conversation_id: i64) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE bots SET current_conversation_id = ?1
             WHERE id = (SELECT bot_id FROM conversations WHERE id = ?1)
                AND current_conversation_id IS NOT ?1",
            params![conversation_id],
        )?;
        Ok(changed > 0)
    }

    fn insert_conversation(&self, bot_id: i64) -> Result<Option<i64>, StoreError> {
        let inserted = self.connection.execute(
            "INSERT INTO conversations (bot_id, provider, cwd, created_at_ms)
             SELECT id, provider, workspace, ?2 FROM bots WHERE id = ?1",
            params![bot_id, now_ms()],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let id = self.connection.last_insert_rowid();
        self.connection.execute(
            "UPDATE bots SET current_conversation_id = ?2 WHERE id = ?1",
            params![bot_id, id],
        )?;
        Ok(Some(id))
    }

    pub fn standing(&self, bot_id: i64) -> Result<Standing, StoreError> {
        let standing = self.connection.query_row(
            "SELECT
                (SELECT COUNT(*) FROM events e
                    JOIN conversations c ON c.id = e.conversation_id
                    WHERE c.bot_id = ?1 AND e.id > c.read_event_id
                        AND e.kind IN ('turn_ended', 'turn_failed', 'turn_interrupted')),
                EXISTS (SELECT 1 FROM conversations c
                    JOIN events e ON e.id = (SELECT MAX(id) FROM events
                        WHERE conversation_id = c.id
                            AND kind IN ('turn_ended', 'turn_failed', 'turn_interrupted'))
                    WHERE c.bot_id = ?1 AND e.id > c.read_event_id
                        AND e.kind IN ('turn_failed', 'turn_interrupted')
                        AND NOT EXISTS (SELECT 1 FROM events
                            WHERE conversation_id = c.id AND id > e.id AND kind = 'user_text')),
                COALESCE(
                    (SELECT MAX(e.created_at_ms) FROM events e
                        JOIN conversations c ON c.id = e.conversation_id WHERE c.bot_id = ?1),
                    (SELECT created_at_ms FROM bots WHERE id = ?1),
                    0)",
            params![bot_id],
            |row| {
                Ok(Standing {
                    unread: row.get(0)?,
                    failed: row.get(1)?,
                    last_activity_ms: row.get(2)?,
                })
            },
        )?;
        Ok(standing)
    }

    /// Moves the read mark of a conversation to its last event, and returns the
    /// bot it belongs to. `None` means the conversation does not exist.
    pub fn mark_read(&self, conversation_id: i64) -> Result<Option<i64>, StoreError> {
        let bot_id = self
            .connection
            .query_row(
                "UPDATE conversations
                 SET read_event_id = COALESCE(
                    (SELECT MAX(id) FROM events WHERE conversation_id = ?1), 0)
                 WHERE id = ?1
                 RETURNING bot_id",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bot_id)
    }

    pub fn mark_bot_read(&self, bot_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE conversations
             SET read_event_id = COALESCE(
                (SELECT MAX(id) FROM events WHERE conversation_id = conversations.id), 0)
             WHERE bot_id = ?1",
            params![bot_id],
        )?;
        Ok(())
    }

    /// Moves a read mark back, before the last turn that ended in the bot's
    /// conversations. `false` means that no turn has ended yet.
    pub fn mark_bot_unread(&self, bot_id: i64) -> Result<bool, StoreError> {
        let last_end = self
            .connection
            .query_row(
                "SELECT e.conversation_id, e.id FROM events e
                 JOIN conversations c ON c.id = e.conversation_id
                 WHERE c.bot_id = ?1
                    AND e.kind IN ('turn_ended', 'turn_failed', 'turn_interrupted')
                 ORDER BY e.id DESC LIMIT 1",
                params![bot_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((conversation_id, event_id)) = last_end else {
            return Ok(false);
        };
        self.connection.execute(
            "UPDATE conversations SET read_event_id = MIN(read_event_id, ?2) WHERE id = ?1",
            params![conversation_id, event_id - 1],
        )?;
        Ok(true)
    }

    /// Gives the conversations stored before bots existed to one new bot. Its
    /// provider and folder are those of the latest conversation, every
    /// conversation counts as read, and each takes its title from its first
    /// message. A provider session started in another folder is not resumed.
    pub(super) fn adopt_conversations(&self) -> Result<(), StoreError> {
        let latest = self
            .connection
            .query_row(
                "SELECT provider, cwd, (SELECT MIN(created_at_ms) FROM conversations)
                 FROM conversations ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((provider, workspace, created_at_ms)) = latest else {
            return Ok(());
        };
        let glyph: String = ADOPTER_NAME.chars().take(1).collect();
        self.connection.execute(
            "INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ADOPTER_NAME,
                ADOPTER_COLOR,
                glyph,
                provider,
                workspace,
                created_at_ms
            ],
        )?;
        let bot_id = self.connection.last_insert_rowid();
        self.connection.execute(
            "UPDATE conversations SET
                bot_id = ?1,
                read_event_id = COALESCE(
                    (SELECT MAX(id) FROM events WHERE conversation_id = conversations.id), 0),
                session_revision = CASE
                    WHEN provider_session_id IS NOT NULL AND provider = ?2 AND cwd = ?3 THEN 1
                END",
            params![bot_id, provider, workspace],
        )?;
        let current = self.summaries(bot_id)?.first().map(|summary| summary.id);
        self.connection.execute(
            "UPDATE bots SET current_conversation_id = ?2 WHERE id = ?1",
            params![bot_id, current],
        )?;

        let first_messages = {
            let mut statement = self.connection.prepare(
                "SELECT conversation_id, payload FROM events
                 WHERE id IN (SELECT MIN(id) FROM events
                    WHERE kind = 'user_text' GROUP BY conversation_id)",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (conversation_id, payload) in first_messages {
            if let Event::UserText(text) = serde_json::from_str(&payload)?
                && let Some(title) = title_of(&text)
            {
                self.connection.execute(
                    "UPDATE conversations SET title = ?2 WHERE id = ?1",
                    params![conversation_id, title],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{Interruption, TurnEnd};
    use rusqlite::Connection;

    use super::super::{SCHEMA_V1, SCHEMA_V2, SCHEMA_VERSION};
    use super::*;

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn new_bot(name: &str) -> NewBot {
        NewBot {
            name: name.to_owned(),
            title: "Researcher".to_owned(),
            instructions: "Answer in one line.".to_owned(),
            avatar: Avatar {
                color: "#10b981".to_owned(),
                glyph: "A".to_owned(),
            },
            provider: "claude".to_owned(),
            model: Some("opus".to_owned()),
            permission_level: PermissionLevel::AskFirst,
            workspace: "/bots/atlas".to_owned(),
        }
    }

    fn version(store: &Store) -> i64 {
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn turn(store: &Store, conversation_id: i64, end: Event) {
        for event in [
            Event::UserText("go".to_owned()),
            Event::MessageText("done".to_owned()),
            end,
        ] {
            store.append(conversation_id, &event).unwrap();
        }
    }

    fn failed() -> Event {
        Event::TurnFailed {
            message: "gone".to_owned(),
        }
    }

    #[test]
    fn a_bot_keeps_every_field_and_starts_with_a_current_conversation() {
        let store = store();
        let created = store.create_bot(&new_bot("Atlas")).unwrap();
        let loaded = store.bot(created.bot.id).unwrap().unwrap();
        assert_eq!(loaded.bot, created.bot);
        assert_eq!(loaded.bot.name, "Atlas");
        assert_eq!(loaded.bot.title, "Researcher");
        assert_eq!(loaded.bot.instructions, "Answer in one line.");
        assert_eq!(loaded.bot.avatar.color, "#10b981");
        assert_eq!(loaded.bot.avatar.glyph, "A");
        assert_eq!(loaded.bot.provider, "claude");
        assert_eq!(loaded.bot.model.as_deref(), Some("opus"));
        assert_eq!(loaded.bot.permission_level, PermissionLevel::AskFirst);
        assert_eq!(loaded.bot.workspace, "/bots/atlas");
        assert!(!loaded.bot.pinned && !loaded.bot.hidden);
        assert!(loaded.bot.created_at_ms > 0);

        let conversation = store
            .conversation(loaded.current_conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(conversation.bot_id, loaded.bot.id);
        assert_eq!(
            store.conversation_ids(loaded.bot.id).unwrap(),
            [conversation.id]
        );
        assert_eq!(store.bots().unwrap().len(), 1);
    }

    #[test]
    fn a_patch_changes_only_the_fields_it_holds() {
        let store = store();
        let id = store.create_bot(&new_bot("Atlas")).unwrap().bot.id;
        let patch = BotPatch {
            name: Some("Orion".to_owned()),
            pinned: Some(true),
            permission_level: Some(PermissionLevel::FullAccess),
            ..BotPatch::default()
        };
        let updated = store.update_bot(id, &patch).unwrap().unwrap();
        assert_eq!(updated.bot.name, "Orion");
        assert!(updated.bot.pinned);
        assert_eq!(updated.bot.permission_level, PermissionLevel::FullAccess);
        assert_eq!(updated.bot.title, "Researcher");
        assert_eq!(updated.bot.model.as_deref(), Some("opus"));
        assert!(store.update_bot(id + 1, &patch).unwrap().is_none());
    }

    #[test]
    fn only_the_session_settings_start_a_new_revision() {
        let store = store();
        let created = store.create_bot(&new_bot("Atlas")).unwrap();
        let id = created.bot.id;
        let revision = |patch: BotPatch| {
            store
                .update_bot(id, &patch)
                .unwrap()
                .unwrap()
                .session_revision
        };
        let start = created.session_revision;
        let same = revision(BotPatch {
            name: Some("Orion".to_owned()),
            permission_level: Some(PermissionLevel::FullAccess),
            instructions: Some("Answer in one line.".to_owned()),
            ..BotPatch::default()
        });
        assert_eq!(same, start);
        let instructions = revision(BotPatch {
            instructions: Some("Answer in French.".to_owned()),
            ..BotPatch::default()
        });
        assert_eq!(instructions, start + 1);
        let model = revision(BotPatch {
            model: Some(None),
            ..BotPatch::default()
        });
        assert_eq!(model, start + 2);
        let workspace = revision(BotPatch {
            workspace: Some("/elsewhere".to_owned()),
            ..BotPatch::default()
        });
        assert_eq!(workspace, start + 3);
    }

    #[test]
    fn deleting_a_bot_removes_its_conversations_events_and_queue_only() {
        let store = store();
        let kept = store.create_bot(&new_bot("Kept")).unwrap();
        let gone = store.create_bot(&new_bot("Gone")).unwrap();
        let second = store.new_conversation(gone.bot.id).unwrap().unwrap().id;
        turn(
            &store,
            kept.current_conversation_id,
            Event::TurnEnded(TurnEnd::Completed),
        );
        turn(
            &store,
            gone.current_conversation_id,
            Event::TurnEnded(TurnEnd::Completed),
        );
        store.enqueue(second, "later").unwrap();

        assert!(store.delete_bot(gone.bot.id).unwrap());
        assert!(store.bot(gone.bot.id).unwrap().is_none());
        assert!(
            store
                .conversation(gone.current_conversation_id)
                .unwrap()
                .is_none()
        );
        assert!(store.conversation(second).unwrap().is_none());
        assert!(
            store
                .events(gone.current_conversation_id)
                .unwrap()
                .is_empty()
        );
        assert!(store.queue(second).unwrap().messages.is_empty());
        assert_eq!(store.events(kept.current_conversation_id).unwrap().len(), 3);
        assert!(!store.delete_bot(gone.bot.id).unwrap());
    }

    #[test]
    fn a_new_conversation_becomes_current_and_the_previous_one_stays() {
        let store = store();
        let bot = store.create_bot(&new_bot("Atlas")).unwrap();
        let first = bot.current_conversation_id;
        let second = store.new_conversation(bot.bot.id).unwrap().unwrap();
        assert_eq!(second.bot_id, bot.bot.id);
        assert_eq!(
            store
                .bot(bot.bot.id)
                .unwrap()
                .unwrap()
                .current_conversation_id,
            second.id
        );
        assert_eq!(
            store.conversation_ids(bot.bot.id).unwrap(),
            [first, second.id]
        );
        assert!(store.new_conversation(bot.bot.id + 1).unwrap().is_none());

        // An earlier conversation becomes the current one again on request.
        let current = || {
            store
                .bot(bot.bot.id)
                .unwrap()
                .unwrap()
                .current_conversation_id
        };
        assert!(!store.make_current(second.id).unwrap());
        assert!(store.make_current(first).unwrap());
        assert_eq!(current(), first);
        assert!(!store.make_current(999).unwrap());
        assert_eq!(current(), first);
    }

    #[test]
    fn unread_counts_the_turns_that_ended_after_the_read_mark() {
        let store = store();
        let bot = store.create_bot(&new_bot("Atlas")).unwrap();
        let (id, first) = (bot.bot.id, bot.current_conversation_id);
        let second = store.new_conversation(id).unwrap().unwrap().id;
        assert_eq!(store.standing(id).unwrap().unread, 0);

        turn(&store, first, Event::TurnEnded(TurnEnd::Completed));
        turn(&store, first, Event::TurnEnded(TurnEnd::Cancelled));
        turn(&store, second, Event::TurnEnded(TurnEnd::Completed));
        // A turn that still runs is not unread yet.
        store
            .append(second, &Event::UserText("more".to_owned()))
            .unwrap();
        store
            .append(second, &Event::MessageText("partial".to_owned()))
            .unwrap();
        let standing = store.standing(id).unwrap();
        assert_eq!(standing.unread, 3);
        assert!(!standing.failed);
        assert!(standing.last_activity_ms >= bot.bot.created_at_ms);

        assert_eq!(store.mark_read(first).unwrap(), Some(id));
        assert_eq!(store.standing(id).unwrap().unread, 1);
        store.mark_bot_read(id).unwrap();
        assert_eq!(store.standing(id).unwrap().unread, 0);
    }

    #[test]
    fn a_failed_or_interrupted_last_turn_shows_until_it_is_read_or_followed() {
        let store = store();
        let bot = store.create_bot(&new_bot("Atlas")).unwrap();
        let (id, conversation) = (bot.bot.id, bot.current_conversation_id);
        turn(&store, conversation, failed());
        assert!(store.standing(id).unwrap().failed);

        store.mark_read(conversation).unwrap();
        assert!(!store.standing(id).unwrap().failed);

        turn(
            &store,
            conversation,
            Event::TurnInterrupted {
                cause: Interruption::DaemonRestarted,
            },
        );
        assert!(store.standing(id).unwrap().failed);
        // The next message starts a turn: the failure is behind.
        store
            .append(conversation, &Event::UserText("again".to_owned()))
            .unwrap();
        assert!(!store.standing(id).unwrap().failed);
        store
            .append(conversation, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
        let standing = store.standing(id).unwrap();
        assert!(!standing.failed);
        assert_eq!(standing.unread, 2);
    }

    #[test]
    fn marking_a_bot_unread_moves_the_mark_before_its_last_ended_turn() {
        let store = store();
        let bot = store.create_bot(&new_bot("Atlas")).unwrap();
        let (id, conversation) = (bot.bot.id, bot.current_conversation_id);
        assert!(!store.mark_bot_unread(id).unwrap());

        turn(&store, conversation, Event::TurnEnded(TurnEnd::Completed));
        turn(&store, conversation, Event::TurnEnded(TurnEnd::Completed));
        store.mark_bot_read(id).unwrap();
        assert!(store.mark_bot_unread(id).unwrap());
        assert_eq!(store.standing(id).unwrap().unread, 1);
        // Asking again does not move the mark forward or further back.
        assert!(store.mark_bot_unread(id).unwrap());
        assert_eq!(store.standing(id).unwrap().unread, 1);
    }

    #[test]
    fn a_read_mark_for_a_missing_conversation_changes_nothing() {
        let store = store();
        let bot = store.create_bot(&new_bot("Atlas")).unwrap();
        turn(
            &store,
            bot.current_conversation_id,
            Event::TurnEnded(TurnEnd::Completed),
        );
        assert_eq!(store.mark_read(999).unwrap(), None);
        assert_eq!(store.standing(bot.bot.id).unwrap().unread, 1);
    }

    /// A database as the daemon before bots left it, at `version` 1 or 2.
    fn old_database(version: i64) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        if version >= 2 {
            connection.execute_batch(SCHEMA_V2).unwrap();
            connection.pragma_update(None, "user_version", 2).unwrap();
        }
        let conversation = "INSERT INTO conversations
            (provider, command, cwd, provider_session_id, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)";
        connection
            .execute(
                conversation,
                params!["custom", "old-agent", "/old", "session-1", 7],
            )
            .unwrap();
        connection
            .execute(
                conversation,
                params!["claude", None::<String>, "/work", "session-2", 9],
            )
            .unwrap();
        connection
            .execute(
                conversation,
                params!["claude", None::<String>, "/work", None::<String>, 11],
            )
            .unwrap();
        let event = "INSERT INTO events (conversation_id, kind, payload, created_at_ms)
            VALUES (?1, ?2, ?3, ?4)";
        for (conversation_id, kind, payload, at) in [
            (
                1,
                "user_text",
                Event::UserText("Plan the trip".to_owned()),
                8,
            ),
            (1, "turn_ended", Event::TurnEnded(TurnEnd::Completed), 9),
            (
                2,
                "user_text",
                Event::UserText("\n  Fix the build\nnow".to_owned()),
                20,
            ),
            (2, "turn_failed", failed(), 21),
        ] {
            connection
                .execute(
                    event,
                    params![
                        conversation_id,
                        kind,
                        serde_json::to_string(&payload).unwrap(),
                        at
                    ],
                )
                .unwrap();
        }
        connection
    }

    #[test]
    fn older_databases_migrate_and_one_new_bot_owns_every_conversation() {
        for from in [1, 2] {
            let store = Store::from_connection(old_database(from)).unwrap();
            assert_eq!(version(&store), SCHEMA_VERSION);
            let bots = store.bots().unwrap();
            assert_eq!(bots.len(), 1, "from version {from}");
            let owner = &bots[0];
            assert_eq!(owner.bot.name, "Crow");
            assert_eq!(owner.bot.provider, "claude");
            assert_eq!(owner.bot.workspace, "/work");
            assert_eq!(owner.bot.permission_level, PermissionLevel::AskFirst);
            assert_eq!(owner.bot.created_at_ms, 7);
            // The conversation with the latest activity is the current one.
            assert_eq!(owner.current_conversation_id, 2);
            assert_eq!(store.conversation_ids(owner.bot.id).unwrap(), [1, 2, 3]);

            // Every row is kept, and what was stored counts as read.
            assert_eq!(store.events(1).unwrap().len(), 2);
            assert_eq!(store.events(2).unwrap().len(), 2);
            let standing = store.standing(owner.bot.id).unwrap();
            assert_eq!(standing.unread, 0);
            assert!(!standing.failed);

            let titles: Vec<Option<String>> = [1, 2, 3]
                .iter()
                .map(|id| store.conversation(*id).unwrap().unwrap().title)
                .collect();
            assert_eq!(
                titles,
                [
                    Some("Plan the trip".to_owned()),
                    Some("Fix the build".to_owned()),
                    None
                ]
            );

            // A session started with another provider or folder is not resumed.
            let sessions: Vec<(Option<String>, Option<i64>)> = [1, 2, 3]
                .iter()
                .map(|id| {
                    let conversation = store.conversation(*id).unwrap().unwrap();
                    (
                        conversation.provider_session_id,
                        conversation.session_revision,
                    )
                })
                .collect();
            assert_eq!(
                sessions,
                [
                    (Some("session-1".to_owned()), None),
                    (Some("session-2".to_owned()), Some(owner.session_revision)),
                    (None, None)
                ]
            );
        }
    }

    #[test]
    fn an_empty_older_database_migrates_to_no_bot() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        let store = Store::from_connection(connection).unwrap();
        assert_eq!(version(&store), SCHEMA_VERSION);
        assert!(store.bots().unwrap().is_empty());
    }

    #[test]
    fn a_bot_migration_that_fails_halfway_leaves_the_database_as_it_was() {
        for from in [1, 2] {
            let connection = old_database(from);
            // The step that adds bots fails after the earlier steps ran.
            connection
                .execute_batch("CREATE TABLE bots (blocker INTEGER)")
                .unwrap();
            let store = Store { connection };
            assert!(store.migrate().is_err());
            assert_eq!(version(&store), from);
            let columns = |name: &str| -> i64 {
                store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = ?1",
                        params![name],
                        |row| row.get(0),
                    )
                    .unwrap()
            };
            assert_eq!(columns("bot_id"), 0);
            assert_eq!(columns("queue_paused"), i64::from(from >= 2));
            let rows: i64 = store
                .connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM conversations) + (SELECT COUNT(*) FROM events)",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 7);

            // Once the cause is gone, the same database migrates.
            store.connection.execute_batch("DROP TABLE bots").unwrap();
            store.migrate().unwrap();
            assert_eq!(version(&store), SCHEMA_VERSION);
            assert_eq!(store.bots().unwrap().len(), 1);
        }
    }
}
