//! What the database keeps about providers: the session each provider holds
//! for a conversation, and the settings of each provider.

use std::collections::BTreeMap;

use crowbot_protocol::{Bot, Event, UserMessage};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError};
use crate::digest::sha256_hex;

/// The session a provider holds for one conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSession {
    pub provider: String,
    pub session_id: String,
    /// The session settings of the bot when the session started.
    pub fingerprint: String,
    /// The last event of the conversation that the session knows about.
    pub last_event_id: i64,
    /// The version of each section of the system prompt the session has
    /// seen, by the section's id.
    pub sections: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderSetting {
    /// The executable the user set in place of the runtime the daemon installs.
    pub executable: Option<String>,
    /// A sign-in started from the daemon ended well.
    pub signed_in: bool,
}

/// What a provider session is started with. A stored session is resumed only
/// when the bot still has the same.
pub fn session_fingerprint(bot: &Bot) -> String {
    fingerprint_of(
        &bot.instructions,
        bot.model.as_deref(),
        bot.effort.as_deref(),
        &bot.workspace,
    )
}

fn fingerprint_of(
    instructions: &str,
    model: Option<&str>,
    effort: Option<&str>,
    workspace: &str,
) -> String {
    let settings = serde_json::json!([instructions, model, effort, workspace]);
    sha256_hex(settings.to_string().as_bytes())
}

const SESSION_COLUMNS: &str = "provider, session_id, fingerprint, last_event_id, sections";

fn session_row(row: &Row<'_>) -> rusqlite::Result<ProviderSession> {
    let sections: String = row.get(4)?;
    Ok(ProviderSession {
        provider: row.get(0)?,
        session_id: row.get(1)?,
        fingerprint: row.get(2)?,
        last_event_id: row.get(3)?,
        // Versions that cannot be read count as none seen: the next prompt
        // then carries every section in its note.
        sections: serde_json::from_str(&sections).unwrap_or_default(),
    })
}

impl Store {
    pub fn provider_session(
        &self,
        conversation_id: i64,
        provider: &str,
    ) -> Result<Option<ProviderSession>, StoreError> {
        let session = self
            .connection
            .query_row(
                &format!(
                    "SELECT {SESSION_COLUMNS} FROM provider_sessions
                     WHERE conversation_id = ?1 AND provider = ?2"
                ),
                params![conversation_id, provider],
                session_row,
            )
            .optional()?;
        Ok(session)
    }

    /// The session of the provider that ran the conversation last.
    pub fn last_provider_session(
        &self,
        conversation_id: i64,
    ) -> Result<Option<ProviderSession>, StoreError> {
        let session = self
            .connection
            .query_row(
                &format!(
                    "SELECT {SESSION_COLUMNS} FROM provider_sessions
                     WHERE conversation_id = ?1
                     ORDER BY last_event_id DESC, rowid DESC LIMIT 1"
                ),
                params![conversation_id],
                session_row,
            )
            .optional()?;
        Ok(session)
    }

    /// Stores the session a provider started or resumed. It knows the
    /// conversation up to its last event.
    pub fn set_provider_session(
        &self,
        conversation_id: i64,
        provider: &str,
        session_id: &str,
        fingerprint: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO provider_sessions
                (conversation_id, provider, session_id, fingerprint, last_event_id)
             VALUES (?1, ?2, ?3, ?4,
                COALESCE((SELECT MAX(id) FROM events WHERE conversation_id = ?1), 0))
             ON CONFLICT (conversation_id, provider) DO UPDATE SET
                session_id = excluded.session_id,
                fingerprint = excluded.fingerprint,
                last_event_id = excluded.last_event_id",
            params![conversation_id, provider, session_id, fingerprint],
        )?;
        Ok(())
    }

    /// Stores the versions of the sections of the system prompt that the
    /// session of a provider has seen.
    pub fn set_session_sections(
        &self,
        conversation_id: i64,
        provider: &str,
        sections: &BTreeMap<String, String>,
    ) -> Result<(), StoreError> {
        let sections = serde_json::to_string(sections).map_err(StoreError::from)?;
        self.connection.execute(
            "UPDATE provider_sessions SET sections = ?3
             WHERE conversation_id = ?1 AND provider = ?2",
            params![conversation_id, provider, sections],
        )?;
        Ok(())
    }

    /// The session of the provider knows the conversation up to its last event.
    pub fn touch_provider_session(
        &self,
        conversation_id: i64,
        provider: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE provider_sessions
             SET last_event_id = COALESCE(
                (SELECT MAX(id) FROM events WHERE conversation_id = ?1), last_event_id)
             WHERE conversation_id = ?1 AND provider = ?2",
            params![conversation_id, provider],
        )?;
        Ok(())
    }

    /// What was said after the event `after` and before the last message of
    /// the user, oldest first. The most recent events are kept, up to the
    /// one that brings their text past `characters`.
    pub fn handoff_events(
        &self,
        conversation_id: i64,
        after: i64,
        characters: usize,
    ) -> Result<Vec<Event>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM events
             WHERE conversation_id = ?1 AND id > ?2
                AND id < COALESCE((SELECT MAX(id) FROM events
                    WHERE conversation_id = ?1 AND kind = 'user_text'), 0)
                AND kind IN ('user_text', 'message_text', 'tool_call_started')
             ORDER BY id DESC",
        )?;
        let mut rows = statement.query(params![conversation_id, after])?;
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

    pub fn provider_setting(&self, provider: &str) -> Result<ProviderSetting, StoreError> {
        let setting = self
            .connection
            .query_row(
                "SELECT executable, signed_in FROM provider_settings WHERE provider = ?1",
                params![provider],
                |row| {
                    Ok(ProviderSetting {
                        executable: row.get(0)?,
                        signed_in: row.get(1)?,
                    })
                },
            )
            .optional()?;
        Ok(setting.unwrap_or_default())
    }

    pub fn set_provider_executable(
        &self,
        provider: &str,
        executable: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO provider_settings (provider, executable) VALUES (?1, ?2)
             ON CONFLICT (provider) DO UPDATE SET executable = excluded.executable",
            params![provider, executable],
        )?;
        Ok(())
    }

    pub fn set_provider_signed_in(
        &self,
        provider: &str,
        signed_in: bool,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO provider_settings (provider, signed_in) VALUES (?1, ?2)
             ON CONFLICT (provider) DO UPDATE SET signed_in = excluded.signed_in",
            params![provider, signed_in],
        )?;
        Ok(())
    }

    /// Moves the provider session each conversation held before a
    /// conversation could hold one for each provider. A session started with
    /// other settings than the bot has now was not resumed, and is left out.
    pub(super) fn adopt_sessions(&self) -> Result<(), StoreError> {
        let held = {
            let mut statement = self.connection.prepare(
                "SELECT c.id, c.provider_session_id, b.provider, b.instructions, b.model,
                    b.workspace
                 FROM conversations c JOIN bots b ON b.id = c.bot_id
                 WHERE c.provider_session_id IS NOT NULL
                    AND c.session_revision = b.session_revision",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        fingerprint_of(
                            &row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?.as_deref(),
                            None,
                            &row.get::<_, String>(5)?,
                        ),
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (conversation_id, session_id, provider, fingerprint) in held {
            self.set_provider_session(conversation_id, &provider, &session_id, &fingerprint)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{Avatar, BotPatch, PermissionLevel};
    use rusqlite::Connection;

    use super::super::NewBot;
    use super::*;

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn conversation(store: &Store) -> (i64, Bot) {
        let row = store
            .create_bot(&NewBot {
                name: "Atlas".to_owned(),
                title: String::new(),
                instructions: "Answer in one line.".to_owned(),
                avatar: Avatar {
                    color: "#10b981".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/bots/atlas".to_owned(),
            })
            .unwrap();
        (row.current_conversation_id, row.bot)
    }

    #[test]
    fn a_conversation_holds_one_session_for_each_provider() {
        let store = store();
        let (id, bot) = conversation(&store);
        assert_eq!(store.provider_session(id, "claude").unwrap(), None);
        assert_eq!(store.last_provider_session(id).unwrap(), None);

        store
            .append(id, &Event::user_text("one".to_owned()))
            .unwrap();
        let fingerprint = session_fingerprint(&bot);
        store
            .set_provider_session(id, "claude", "claude-1", &fingerprint)
            .unwrap();
        let first = store
            .append(id, &Event::user_text("two".to_owned()))
            .unwrap();
        store
            .set_provider_session(id, "codex", "codex-1", &fingerprint)
            .unwrap();

        let claude = store.provider_session(id, "claude").unwrap().unwrap();
        assert_eq!(claude.session_id, "claude-1");
        assert_eq!(claude.last_event_id, first - 1);
        let last = store.last_provider_session(id).unwrap().unwrap();
        assert_eq!(
            (last.provider.as_str(), last.last_event_id),
            ("codex", first)
        );

        store
            .append(id, &Event::user_text("three".to_owned()))
            .unwrap();
        store.touch_provider_session(id, "claude").unwrap();
        let last = store.last_provider_session(id).unwrap().unwrap();
        assert_eq!(last.provider, "claude");
        assert_eq!(last.session_id, "claude-1");
    }

    #[test]
    fn a_fingerprint_follows_what_a_session_is_started_with() {
        let store = store();
        let (_, bot) = conversation(&store);
        let start = session_fingerprint(&bot);
        let patched = |patch: BotPatch| {
            session_fingerprint(&store.update_bot(bot.id, &patch).unwrap().unwrap().bot)
        };
        assert_eq!(
            patched(BotPatch {
                name: Some("Orion".to_owned()),
                ..BotPatch::default()
            }),
            start
        );
        let with_effort = patched(BotPatch {
            effort: Some(Some("high".to_owned())),
            ..BotPatch::default()
        });
        assert_ne!(with_effort, start);
        assert_eq!(
            patched(BotPatch {
                effort: Some(None),
                ..BotPatch::default()
            }),
            start
        );
    }

    #[test]
    fn a_handoff_holds_what_was_said_before_the_last_message_of_the_user() {
        let store = store();
        let (id, _) = conversation(&store);
        let said = |event: Event| store.append(id, &event).unwrap();
        said(Event::user_text("first question".to_owned()));
        let answered = said(Event::MessageText("first answer".to_owned()));
        said(Event::ThoughtText("a thought".to_owned()));
        said(Event::user_text("second question".to_owned()));
        said(Event::MessageText("second answer".to_owned()));
        said(Event::user_text("the message being sent".to_owned()));

        let texts = |after: i64, characters: usize| {
            store
                .handoff_events(id, after, characters)
                .unwrap()
                .into_iter()
                .map(|event| match event {
                    Event::UserText(UserMessage { text, .. }) | Event::MessageText(text) => text,
                    other => panic!("unexpected {other:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            texts(0, 1000),
            [
                "first question",
                "first answer",
                "second question",
                "second answer"
            ]
        );
        assert_eq!(texts(answered, 1000), ["second question", "second answer"]);
        assert_eq!(texts(0, 5), ["second answer"]);
        assert_eq!(texts(0, 14), ["second question", "second answer"]);
    }

    #[test]
    fn a_provider_keeps_its_executable_and_its_sign_in() {
        let store = store();
        assert_eq!(
            store.provider_setting("codex").unwrap(),
            ProviderSetting::default()
        );
        store
            .set_provider_executable("codex", Some("/opt/codex-acp"))
            .unwrap();
        store.set_provider_signed_in("codex", true).unwrap();
        assert_eq!(
            store.provider_setting("codex").unwrap(),
            ProviderSetting {
                executable: Some("/opt/codex-acp".to_owned()),
                signed_in: true,
            }
        );
        store.set_provider_executable("codex", None).unwrap();
        let cleared = store.provider_setting("codex").unwrap();
        assert_eq!((cleared.executable, cleared.signed_in), (None, true));
        assert_eq!(
            store.provider_setting("grok").unwrap(),
            ProviderSetting::default()
        );
    }
}
