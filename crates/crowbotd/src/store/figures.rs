//! What skills change (US-029): each turn that ended while the skills index
//! listed a skill, or that loaded one, with how it ended and the versions it
//! loaded. The turn's usage is the row of `turn_usage` of the same turn, so
//! the figures join the two.

use crowbot_protocol::{BotSkillFigures, Cost, Figure, SkillFigures, SkillVersionFigures};
use rusqlite::params;

use super::{Store, StoreError, TurnEndKind, now_ms};

/// A version of a skill that a turn loaded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SkillUse {
    pub skill_id: i64,
    pub version: String,
}

fn share(part: i64, whole: i64) -> Option<Figure> {
    // The counts are of turns: far below what an f64 holds exactly.
    #[expect(clippy::cast_precision_loss)]
    let share = (whole > 0).then(|| Figure(part as f64 / whole as f64));
    share
}

impl Store {
    /// Records how the last turn of a conversation ended and the skills it
    /// loaded. A turn that ended while the index listed no skill, and that
    /// loaded none, leaves no row.
    pub fn record_skill_turn(
        &self,
        conversation_id: i64,
        indexed: u32,
        end: TurnEndKind,
        uses: &[SkillUse],
    ) -> Result<(), StoreError> {
        if indexed == 0 && uses.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO skill_turns
                (turn_event_id, bot_id, conversation_id, indexed, end_kind, created_at_ms)
             SELECT e.id, c.bot_id, c.id, ?2, ?3, ?4 FROM conversations c
             JOIN events e ON e.id = (SELECT MAX(id) FROM events
                WHERE conversation_id = c.id AND kind = 'user_text')
             WHERE c.id = ?1",
            params![conversation_id, indexed, end.name(), now_ms()],
        )?;
        if inserted > 0 {
            let turn = transaction.last_insert_rowid();
            for used in uses {
                transaction.execute(
                    "INSERT OR IGNORE INTO skill_uses (turn_event_id, skill_id, version)
                     SELECT ?1, id, ?3 FROM skills WHERE id = ?2",
                    params![turn, used.skill_id, used.version],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// The figures of a skill, per version it had. `None` means the bot has
    /// no skill of that name.
    pub fn skill_figures(
        &self,
        bot_id: i64,
        name: &str,
    ) -> Result<Option<SkillFigures>, StoreError> {
        let Some(history) = self.skill_history(bot_id, name)? else {
            return Ok(None);
        };
        let Some(skill) = self.skill(bot_id, name)? else {
            return Ok(None);
        };
        let mut hashes: Vec<String> = Vec::new();
        for version in history {
            if !hashes.contains(&version.hash) {
                hashes.push(version.hash);
            }
        }
        let mut versions = Vec::new();
        for hash in hashes {
            let (turns, completed, with_tokens, tokens): (i64, i64, i64, Option<i64>) =
                self.connection.query_row(
                    "SELECT COUNT(*), COUNT(CASE WHEN t.end_kind = 'completed' THEN 1 END),
                        COUNT(u.total_tokens), SUM(u.total_tokens)
                     FROM skill_uses s
                     JOIN skill_turns t ON t.turn_event_id = s.turn_event_id
                     LEFT JOIN turn_usage u ON u.turn_event_id = s.turn_event_id
                     WHERE s.skill_id = ?1 AND s.version = ?2",
                    params![skill.id, hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
            let mut statement = self.connection.prepare(
                "SELECT u.cost_currency, AVG(u.turn_cost) FROM skill_uses s
                 JOIN turn_usage u ON u.turn_event_id = s.turn_event_id
                 WHERE s.skill_id = ?1 AND s.version = ?2
                    AND u.turn_cost IS NOT NULL AND u.cost_currency IS NOT NULL
                 GROUP BY u.cost_currency ORDER BY u.cost_currency",
            )?;
            let mean_costs = statement
                .query_map(params![skill.id, hash], |row| {
                    Ok(Cost {
                        currency: row.get(0)?,
                        amount: row.get(1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            versions.push(SkillVersionFigures {
                hash,
                turns: u32::try_from(turns).unwrap_or(u32::MAX),
                completed: share(completed, turns),
                mean_tokens: tokens.and_then(|tokens| share(tokens, with_tokens)),
                mean_costs,
            });
        }
        Ok(Some(SkillFigures {
            bot_id,
            name: skill.name,
            versions,
        }))
    }

    /// How often the bot's turns loaded a skill while its index listed one.
    pub fn bot_skill_figures(&self, bot_id: i64) -> Result<BotSkillFigures, StoreError> {
        let (turns, loaded): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COUNT(CASE WHEN EXISTS (SELECT 1 FROM skill_uses s
                    WHERE s.turn_event_id = t.turn_event_id) THEN 1 END)
             FROM skill_turns t WHERE t.bot_id = ?1 AND t.indexed > 0",
            params![bot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(BotSkillFigures {
            bot_id,
            turns: u32::try_from(turns).unwrap_or(u32::MAX),
            loaded: u32::try_from(loaded).unwrap_or(u32::MAX),
            share: share(loaded, turns),
        })
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{
        Avatar, Event, PermissionLevel, SkillAuthor, SkillChangeKind, TurnEnd, UserMessage,
    };

    use super::*;
    use crate::skills::format::Version;
    use crate::store::{Expected, NewBot, Reported, SkillPlan, SkillWriter, Tokens};

    /// A turn of the conversation that loaded `uses`, with the tokens and
    /// the cost of the session its agent reported, if any.
    fn turn(
        store: &Store,
        conversation: i64,
        session_cost: Option<f64>,
        tokens: Option<u64>,
        end: TurnEndKind,
        uses: &[SkillUse],
    ) {
        store
            .append(conversation, &Event::UserText(UserMessage::new("go")))
            .unwrap();
        let reported = Reported {
            tokens: tokens.map(|total| Tokens {
                input: total,
                output: 0,
                total,
            }),
            session_cost: session_cost.map(|amount| Cost {
                amount,
                currency: "USD".to_owned(),
            }),
            ..Reported::default()
        };
        store
            .record_usage(conversation, "claude", Some("s"), &reported)
            .unwrap();
        store.record_skill_turn(conversation, 1, end, uses).unwrap();
        store
            .append(conversation, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
    }

    #[test]
    fn a_version_s_figures_join_the_usage_its_turns_reported() {
        let store =
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let bot = store
            .create_bot(&NewBot {
                name: "Atlas".to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#3b82f6".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/".to_owned(),
            })
            .unwrap();
        let (bot_id, conversation) = (bot.bot.id, bot.current_conversation_id);
        let version = Version::of_document(
            "---\nname: deploy\ndescription: Deploys.\n---\nRun make deploy.\n",
        );
        store
            .change_skills(
                &SkillWriter {
                    author: SkillAuthor::Bot,
                    bot_id,
                    conversation_id: None,
                },
                &[SkillPlan {
                    version: Some((version.clone(), SkillChangeKind::Created)),
                    ..SkillPlan::new("deploy", Expected::New)
                }],
            )
            .unwrap();
        let skill = store.skill(bot_id, "deploy").unwrap().unwrap();
        let used = [SkillUse {
            skill_id: skill.id,
            version: skill.version.clone(),
        }];

        // Two turns load the version and report usage, the session's cost
        // growing by 0.25 then by 0.75; a third reports nothing and fails; a
        // fourth loads nothing.
        turn(
            &store,
            conversation,
            Some(0.25),
            Some(100),
            TurnEndKind::Completed,
            &used,
        );
        turn(
            &store,
            conversation,
            Some(1.0),
            Some(300),
            TurnEndKind::Completed,
            &used,
        );
        turn(&store, conversation, None, None, TurnEndKind::Failed, &used);
        turn(
            &store,
            conversation,
            None,
            None,
            TurnEndKind::Completed,
            &[],
        );

        let figures = store.skill_figures(bot_id, "deploy").unwrap().unwrap();
        let [only] = &figures.versions[..] else {
            panic!("{figures:?}")
        };
        assert_eq!(only.hash, version.hash());
        assert_eq!(only.turns, 3);
        assert_eq!(only.completed, Some(Figure(2.0 / 3.0)));
        assert_eq!(only.mean_tokens, Some(Figure(200.0)));
        assert_eq!(
            only.mean_costs,
            [Cost {
                currency: "USD".to_owned(),
                amount: 0.5
            }]
        );
        let bot = store.bot_skill_figures(bot_id).unwrap();
        assert_eq!((bot.turns, bot.loaded), (4, 3));
        assert_eq!(bot.share, Some(Figure(0.75)));
        assert_eq!(store.skill_figures(bot_id, "nothing").unwrap(), None);
    }
}
