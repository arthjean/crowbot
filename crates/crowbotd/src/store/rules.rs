//! The approval rules a person gave each bot. Only a client message writes
//! one (`daemon.rs`); no daemon tool, prompt, or file reaches this module.

use crowbot_protocol::{ApprovalRule, RuleDraft, RuleKind};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, now_ms};

const RULE_COLUMNS: &str = "id, bot_id, kind, program, args, created_at_ms";

fn kind_name(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Refuse => "refuse",
        RuleKind::Allow => "allow",
    }
}

fn rule_row(row: &Row<'_>) -> rusqlite::Result<(ApprovalRule, String, String)> {
    Ok((
        ApprovalRule {
            id: row.get(0)?,
            bot_id: row.get(1)?,
            kind: RuleKind::Refuse,
            program: row.get(3)?,
            args: Vec::new(),
            created_at_ms: row.get(5)?,
        },
        row.get(2)?,
        row.get(4)?,
    ))
}

/// A stored row as a rule. A kind this daemon does not know refuses, so a
/// row it cannot read never allows anything.
fn read_rule(
    (mut rule, kind, args): (ApprovalRule, String, String),
) -> Result<ApprovalRule, StoreError> {
    rule.kind = if kind == "allow" {
        RuleKind::Allow
    } else {
        RuleKind::Refuse
    };
    rule.args = serde_json::from_str(&args)?;
    Ok(rule)
}

impl Store {
    /// The rules of a bot, the oldest first.
    pub fn approval_rules(&self, bot_id: i64) -> Result<Vec<ApprovalRule>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {RULE_COLUMNS} FROM approval_rules WHERE bot_id = ?1 ORDER BY id"
        ))?;
        let rows = statement
            .query_map(params![bot_id], rule_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(read_rule).collect()
    }

    /// Stores a rule for a bot and returns its id; a rule the bot already
    /// has keeps its id. `None` means the bot does not exist.
    pub fn add_approval_rule(
        &self,
        bot_id: i64,
        draft: &RuleDraft,
    ) -> Result<Option<i64>, StoreError> {
        let exists = self
            .connection
            .query_row("SELECT 1 FROM bots WHERE id = ?1", params![bot_id], |_| {
                Ok(())
            })
            .optional()?;
        if exists.is_none() {
            return Ok(None);
        }
        let args = serde_json::to_string(&draft.args)?;
        let kind = kind_name(draft.kind);
        self.connection.execute(
            "INSERT INTO approval_rules (bot_id, kind, program, args, created_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT (bot_id, kind, program, args) DO NOTHING",
            params![bot_id, kind, draft.program, args, now_ms()],
        )?;
        let id = self.connection.query_row(
            "SELECT id FROM approval_rules
                WHERE bot_id = ?1 AND kind = ?2 AND program = ?3 AND args = ?4",
            params![bot_id, kind, draft.program, args],
            |row| row.get(0),
        )?;
        Ok(Some(id))
    }

    /// Removes a rule and returns the bot it belonged to, or `None` when no
    /// such rule exists.
    pub fn delete_approval_rule(&self, rule_id: i64) -> Result<Option<i64>, StoreError> {
        let bot_id = self
            .connection
            .query_row(
                "DELETE FROM approval_rules WHERE id = ?1 RETURNING bot_id",
                params![rule_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bot_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewBot;
    use crowbot_protocol::{Avatar, PermissionLevel};
    use rusqlite::Connection;

    #[test]
    fn a_rule_is_stored_with_its_bot_its_kind_its_program_and_its_arguments() {
        let store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        let bot = store
            .create_bot(&NewBot {
                name: "Ada".to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#10b981".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/bots/ada".to_owned(),
            })
            .unwrap()
            .bot
            .id;
        let draft = RuleDraft {
            kind: RuleKind::Allow,
            program: "cargo".to_owned(),
            args: vec!["test".to_owned(), "--locked".to_owned()],
        };
        let id = store.add_approval_rule(bot, &draft).unwrap().unwrap();
        assert_eq!(store.add_approval_rule(bot, &draft).unwrap(), Some(id));
        let refuse = RuleDraft {
            kind: RuleKind::Refuse,
            program: "git".to_owned(),
            args: vec!["push".to_owned()],
        };
        let refused = store.add_approval_rule(bot, &refuse).unwrap().unwrap();
        let rules = store.approval_rules(bot).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, id);
        assert_eq!(rules[0].bot_id, bot);
        assert_eq!(rules[0].kind, RuleKind::Allow);
        assert_eq!(rules[0].program, "cargo");
        assert_eq!(rules[0].args, ["test", "--locked"]);
        assert_eq!(rules[1].kind, RuleKind::Refuse);
        assert_eq!(store.add_approval_rule(bot + 1, &draft).unwrap(), None);
        assert_eq!(store.delete_approval_rule(refused).unwrap(), Some(bot));
        assert_eq!(store.delete_approval_rule(refused).unwrap(), None);
        assert!(store.delete_bot(bot).unwrap());
        assert!(store.approval_rules(bot).unwrap().is_empty());
    }
}
