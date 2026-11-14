use crowbot_protocol::{Appearance, Preferences, PreferencesPatch, SendShortcut};
use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError};

const APPEARANCE: &str = "appearance";
const SIDEBAR_WIDTH: &str = "sidebar_width";
const SIDEBAR_COLLAPSED: &str = "sidebar_collapsed";
const SEND_SHORTCUT: &str = "send_shortcut";

impl Store {
    fn preference(&self, key: &str) -> Result<Option<String>, StoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    fn set_preference(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO preferences (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn preferences(&self) -> Result<Preferences, StoreError> {
        Ok(Preferences {
            appearance: self
                .preference(APPEARANCE)?
                .map(|name| Appearance::from_name(&name))
                .unwrap_or_default(),
            sidebar_width: self
                .preference(SIDEBAR_WIDTH)?
                .and_then(|width| width.parse().ok()),
            sidebar_collapsed: self.preference(SIDEBAR_COLLAPSED)?.as_deref() == Some("1"),
            send_shortcut: self
                .preference(SEND_SHORTCUT)?
                .map(|name| SendShortcut::from_name(&name))
                .unwrap_or_default(),
        })
    }

    pub fn update_preferences(&self, patch: &PreferencesPatch) -> Result<Preferences, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        if let Some(appearance) = patch.appearance {
            self.set_preference(APPEARANCE, appearance.name())?;
        }
        if let Some(width) = patch.sidebar_width {
            self.set_preference(SIDEBAR_WIDTH, &width.to_string())?;
        }
        if let Some(collapsed) = patch.sidebar_collapsed {
            self.set_preference(SIDEBAR_COLLAPSED, if collapsed { "1" } else { "0" })?;
        }
        if let Some(shortcut) = patch.send_shortcut {
            self.set_preference(SEND_SHORTCUT, shortcut.name())?;
        }
        let preferences = self.preferences()?;
        transaction.commit()?;
        Ok(preferences)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::super::{SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_VERSION};
    use super::*;

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn preferences_nobody_set_are_the_defaults() {
        assert_eq!(store().preferences().unwrap(), Preferences::default());
    }

    #[test]
    fn a_patch_changes_only_the_preferences_it_holds() {
        let store = store();
        let first = store
            .update_preferences(&PreferencesPatch {
                appearance: Some(Appearance::Light),
                sidebar_width: Some(320),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(first.appearance, Appearance::Light);
        assert_eq!(first.sidebar_width, Some(320));
        assert!(!first.sidebar_collapsed);

        let second = store
            .update_preferences(&PreferencesPatch {
                sidebar_collapsed: Some(true),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(second.appearance, Appearance::Light);
        assert_eq!(second.sidebar_width, Some(320));
        assert!(second.sidebar_collapsed);
        assert_eq!(store.preferences().unwrap(), second);

        let third = store
            .update_preferences(&PreferencesPatch {
                appearance: Some(Appearance::System),
                sidebar_collapsed: Some(false),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(third.appearance, Appearance::System);
        assert!(!third.sidebar_collapsed);
    }

    #[test]
    fn the_send_shortcut_is_stored_by_its_name_and_other_patches_leave_it() {
        let store = store();
        assert_eq!(
            store.preferences().unwrap().send_shortcut,
            SendShortcut::Enter
        );
        let changed = store
            .update_preferences(&PreferencesPatch {
                send_shortcut: Some(SendShortcut::ModEnter),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(changed.send_shortcut, SendShortcut::ModEnter);
        assert_eq!(
            store.preference(SEND_SHORTCUT).unwrap().as_deref(),
            Some("mod_enter")
        );
        let later = store
            .update_preferences(&PreferencesPatch {
                sidebar_width: Some(280),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(later.send_shortcut, SendShortcut::ModEnter);
        let back = store
            .update_preferences(&PreferencesPatch {
                send_shortcut: Some(SendShortcut::Enter),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(back.send_shortcut, SendShortcut::Enter);
        assert_eq!(store.preferences().unwrap(), back);
    }

    #[test]
    fn a_stored_value_that_cannot_be_read_gives_the_default() {
        let store = store();
        for (key, value) in [
            (APPEARANCE, "sepia"),
            (SIDEBAR_WIDTH, "wide"),
            (SIDEBAR_COLLAPSED, "maybe"),
            (SEND_SHORTCUT, "double_tap"),
        ] {
            store.set_preference(key, value).unwrap();
        }
        assert_eq!(store.preferences().unwrap(), Preferences::default());
    }

    #[test]
    fn a_version_3_database_gains_preferences_and_keeps_its_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(SCHEMA_V2).unwrap();
        connection.execute_batch(SCHEMA_V3).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        connection
            .execute_batch(
                "INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
                    VALUES ('Atlas', '#3b82f6', 'A', 'claude', '/work', 5);
                 INSERT INTO conversations (bot_id, provider, cwd, created_at_ms)
                    VALUES (1, 'claude', '/work', 6);
                 UPDATE bots SET current_conversation_id = 1;
                 INSERT INTO events (conversation_id, kind, payload, created_at_ms)
                    VALUES (1, 'user_text', '{\"type\":\"user_text\",\"data\":\"kept\"}', 7);",
            )
            .unwrap();
        let store = Store::from_connection(connection).unwrap();
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(store.bots().unwrap().len(), 1);
        assert_eq!(store.events(1).unwrap().len(), 1);
        assert_eq!(store.standing(1).unwrap().preview, "kept");
        assert_eq!(store.preferences().unwrap(), Preferences::default());
        store
            .update_preferences(&PreferencesPatch {
                appearance: Some(Appearance::Dark),
                ..PreferencesPatch::default()
            })
            .unwrap();
        assert_eq!(store.preferences().unwrap().appearance, Appearance::Dark);
    }
}
