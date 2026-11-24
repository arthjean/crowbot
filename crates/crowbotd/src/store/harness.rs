//! What a person allows the daemon to start on its own for each bot, the
//! turns it did not start, and what the turns of each origin consumed during
//! the daemon's local day (decision D4). The origin of a turn is the one its
//! `UserText` event records, so the ledger of the turns that started is the
//! conversation itself.

use std::collections::BTreeMap;

use crowbot_protocol::{
    Consumption, Cost, HarnessSettings, OriginConsumption, TurnOrigin, TurnRefusal,
};
use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError, now_ms};

/// The origin a stored `UserText` records: a message stored as its text
/// alone, the shape of every earlier daemon, is a person's.
const ORIGIN: &str = "CASE WHEN json_valid(e.payload) AND json_type(e.payload, '$.data') = 'object'
    THEN COALESCE(json_extract(e.payload, '$.data.origin'), 'user') ELSE 'user' END";

/// The turns of a bot that started since `?2`, in milliseconds: their event,
/// and their origin.
fn turns_since() -> String {
    format!(
        "SELECT e.id AS turn, {ORIGIN} AS origin FROM events e
         JOIN conversations c ON c.id = e.conversation_id
         WHERE c.bot_id = ?1 AND e.kind = 'user_text' AND e.created_at_ms >= ?2"
    )
}

pub const ORIGINS: [TurnOrigin; 7] = [
    TurnOrigin::User,
    TurnOrigin::Routine,
    TurnOrigin::Review,
    TurnOrigin::Goal,
    TurnOrigin::Delegation,
    TurnOrigin::Retry,
    TurnOrigin::Budget,
];

pub fn origin_name(origin: TurnOrigin) -> String {
    serde_json::to_value(origin)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn refusal_name(cause: TurnRefusal) -> String {
    serde_json::to_value(cause)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// How a turn ended, for the figures of the skills it loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEndKind {
    Completed,
    Failed,
    /// The daemon ended it: a restart, a guard, an agent that did not answer.
    Interrupted,
    /// A person stopped it.
    Stopped,
}

impl TurnEndKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Stopped => "stopped",
        }
    }
}

/// What the daemon-started turns of a bot consumed during the local day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DaemonDay {
    pub turns: u32,
    /// The tokens of those turns whose agent reported some.
    pub tokens: u64,
}

fn to_u64(value: Option<i64>) -> u64 {
    value
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

fn to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl Store {
    /// The settings of a bot. A bot no person set them for has the defaults:
    /// retries on, every other switch off, no limit.
    pub fn harness_settings(&self, bot_id: i64) -> Result<HarnessSettings, StoreError> {
        let stored: Option<String> = self
            .connection
            .query_row(
                "SELECT settings FROM harness_settings WHERE bot_id = ?1",
                params![bot_id],
                |row| row.get(0),
            )
            .optional()?;
        match stored {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(HarnessSettings::default()),
        }
    }

    /// Stores the settings of a bot. `false` means the bot does not exist.
    pub fn set_harness_settings(
        &self,
        bot_id: i64,
        settings: &HarnessSettings,
    ) -> Result<bool, StoreError> {
        let written = self.connection.execute(
            "INSERT INTO harness_settings (bot_id, settings)
             SELECT id, ?2 FROM bots WHERE id = ?1
             ON CONFLICT (bot_id) DO UPDATE SET settings = excluded.settings",
            params![bot_id, serde_json::to_string(settings)?],
        )?;
        Ok(written > 0)
    }

    /// When the daemon's local day started, in milliseconds since the epoch,
    /// and its date. SQLite reads the time zone of the computer.
    pub fn local_day(&self) -> Result<(i64, String), StoreError> {
        let day = self.connection.query_row(
            "SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', 'utc') AS INTEGER) * 1000,
                date('now', 'localtime')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(day)
    }

    /// The turns the daemon started for a bot during the local day, and the
    /// tokens their agents reported.
    pub fn daemon_day(&self, bot_id: i64) -> Result<DaemonDay, StoreError> {
        let (start, _) = self.local_day()?;
        let (turns, tokens) = self.connection.query_row(
            &format!(
                "SELECT COUNT(*), SUM(u.total_tokens) FROM ({}) t
                 LEFT JOIN turn_usage u ON u.turn_event_id = t.turn
                 WHERE t.origin != 'user'",
                turns_since()
            ),
            params![bot_id, start],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )?;
        Ok(DaemonDay {
            turns: to_u32(turns),
            tokens: to_u64(tokens),
        })
    }

    /// Records a turn the daemon did not start. `once_a_day` keeps one
    /// record per origin and cause a day, for a turn that comes due again
    /// at the end of every turn (a review). Returns whether it was recorded.
    pub fn record_refusal(
        &self,
        bot_id: i64,
        conversation_id: Option<i64>,
        origin: TurnOrigin,
        cause: TurnRefusal,
        once_a_day: bool,
    ) -> Result<bool, StoreError> {
        let origin = origin_name(origin);
        let cause = refusal_name(cause);
        if once_a_day {
            let (start, _) = self.local_day()?;
            let recorded: bool = self.connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM turn_refusals
                 WHERE bot_id = ?1 AND origin = ?2 AND cause = ?3 AND created_at_ms >= ?4)",
                params![bot_id, origin, cause, start],
                |row| row.get(0),
            )?;
            if recorded {
                return Ok(false);
            }
        }
        let written = self.connection.execute(
            "INSERT INTO turn_refusals (bot_id, conversation_id, origin, cause, created_at_ms)
             SELECT id, ?2, ?3, ?4, ?5 FROM bots WHERE id = ?1",
            params![bot_id, conversation_id, origin, cause, now_ms()],
        )?;
        Ok(written > 0)
    }

    /// What a bot's turns consumed during the local day, per origin, as
    /// their agents reported it.
    pub fn consumption(&self, bot_id: i64) -> Result<Consumption, StoreError> {
        let (start, day) = self.local_day()?;
        let mut origins: BTreeMap<String, OriginConsumption> = ORIGINS
            .iter()
            .map(|origin| {
                (
                    origin_name(*origin),
                    OriginConsumption {
                        origin: *origin,
                        turns: 0,
                        refused: 0,
                        turns_with_tokens: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        total_tokens: 0,
                        costs: Vec::new(),
                    },
                )
            })
            .collect();
        let turns = format!(
            "SELECT t.origin, COUNT(*), COUNT(u.total_tokens), SUM(u.input_tokens),
                SUM(u.output_tokens), SUM(u.total_tokens)
             FROM ({}) t LEFT JOIN turn_usage u ON u.turn_event_id = t.turn
             GROUP BY t.origin",
            turns_since()
        );
        let mut statement = self.connection.prepare(&turns)?;
        let mut rows = statement.query(params![bot_id, start])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            if let Some(entry) = origins.get_mut(&name) {
                entry.turns = to_u32(row.get(1)?);
                entry.turns_with_tokens = to_u32(row.get(2)?);
                entry.input_tokens = to_u64(row.get(3)?);
                entry.output_tokens = to_u64(row.get(4)?);
                entry.total_tokens = to_u64(row.get(5)?);
            }
        }
        let costs = format!(
            "SELECT t.origin, u.cost_currency, SUM(u.turn_cost)
             FROM ({}) t JOIN turn_usage u ON u.turn_event_id = t.turn
             WHERE u.turn_cost IS NOT NULL AND u.cost_currency IS NOT NULL
             GROUP BY t.origin, u.cost_currency ORDER BY u.cost_currency",
            turns_since()
        );
        let mut statement = self.connection.prepare(&costs)?;
        let mut rows = statement.query(params![bot_id, start])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            if let Some(entry) = origins.get_mut(&name) {
                entry.costs.push(Cost {
                    currency: row.get(1)?,
                    amount: row.get(2)?,
                });
            }
        }
        let mut statement = self.connection.prepare(
            "SELECT origin, COUNT(*) FROM turn_refusals
             WHERE bot_id = ?1 AND created_at_ms >= ?2 GROUP BY origin",
        )?;
        let mut rows = statement.query(params![bot_id, start])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            if let Some(entry) = origins.get_mut(&name) {
                entry.refused = to_u32(row.get(1)?);
            }
        }
        let mut origins: Vec<OriginConsumption> = origins.into_values().collect();
        origins.sort_by_key(|entry| ORIGINS.iter().position(|origin| *origin == entry.origin));
        Ok(Consumption {
            bot_id,
            day,
            origins,
        })
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{Event, TurnEnd, UserMessage};

    use super::*;
    use crate::store::{NewBot, Reported, Tokens};

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
        let conversation = bot.current_conversation_id;
        (store, bot.bot.id, conversation)
    }

    fn turn(store: &Store, conversation: i64, origin: TurnOrigin, tokens: Option<u64>) {
        store
            .append(
                conversation,
                &Event::UserText(UserMessage::new("go").with_origin(origin)),
            )
            .unwrap();
        if let Some(total) = tokens {
            store
                .record_usage(
                    conversation,
                    "claude",
                    Some("s"),
                    &Reported {
                        tokens: Some(Tokens {
                            input: total / 2,
                            output: total - total / 2,
                            total,
                        }),
                        session_cost: Some(Cost {
                            amount: 0.5,
                            currency: "USD".to_owned(),
                        }),
                        ..Reported::default()
                    },
                )
                .unwrap();
        }
        store
            .append(conversation, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
    }

    #[test]
    fn a_bot_no_person_set_has_the_defaults_and_keeps_what_is_set() {
        let (store, bot, _) = store();
        assert_eq!(
            store.harness_settings(bot).unwrap(),
            HarnessSettings::default()
        );
        let mut settings = HarnessSettings::default();
        settings.switches.review = true;
        settings.daily_turn_limit = Some(4);
        assert!(store.set_harness_settings(bot, &settings).unwrap());
        assert_eq!(store.harness_settings(bot).unwrap(), settings);
        assert!(!store.set_harness_settings(bot + 1, &settings).unwrap());
    }

    #[test]
    fn the_day_counts_daemon_turns_and_their_tokens_and_a_person_s_apart() {
        let (store, bot, conversation) = store();
        turn(&store, conversation, TurnOrigin::User, Some(1_000));
        turn(&store, conversation, TurnOrigin::Review, Some(300));
        turn(&store, conversation, TurnOrigin::Retry, None);
        assert_eq!(
            store.daemon_day(bot).unwrap(),
            DaemonDay {
                turns: 2,
                tokens: 300
            }
        );
        store
            .record_refusal(bot, None, TurnOrigin::Review, TurnRefusal::TurnLimit, true)
            .unwrap();
        // A refusal recorded once a day is not recorded again.
        assert!(
            !store
                .record_refusal(bot, None, TurnOrigin::Review, TurnRefusal::TurnLimit, true)
                .unwrap()
        );
        let consumption = store.consumption(bot).unwrap();
        assert_eq!(consumption.origins.len(), ORIGINS.len());
        let of = |origin| {
            consumption
                .origins
                .iter()
                .find(|entry| entry.origin == origin)
                .unwrap()
                .clone()
        };
        assert_eq!(of(TurnOrigin::User).turns, 1);
        assert_eq!(of(TurnOrigin::User).total_tokens, 1_000);
        assert_eq!(of(TurnOrigin::User).costs[0].currency, "USD");
        assert_eq!(of(TurnOrigin::Review).turns, 1);
        assert_eq!(of(TurnOrigin::Review).refused, 1);
        assert_eq!(of(TurnOrigin::Retry).turns_with_tokens, 0);
        assert_eq!(of(TurnOrigin::Goal).turns, 0);
        assert_eq!(consumption.origins[0].origin, TurnOrigin::User);
    }
}
