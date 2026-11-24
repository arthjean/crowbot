//! What agents report that turns consumed: one row per turn that reported
//! anything, kept with the event that started the turn.

use crowbot_protocol::{Cost, TurnUsage};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, now_ms};

/// What an agent reported during one turn. ACP gives the context size and
/// the session's cumulative cost in `UsageUpdate`, and the tokens of a turn
/// in the answer to its prompt (`docs/harness-providers.md`, A7).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reported {
    /// The last context size the agent sent: tokens used, out of the window.
    pub context: Option<(u64, u64)>,
    /// The last cumulative cost of the provider session the agent sent.
    pub session_cost: Option<Cost>,
    pub tokens: Option<Tokens>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

impl Reported {
    pub fn is_empty(&self) -> bool {
        self.context.is_none() && self.session_cost.is_none() && self.tokens.is_none()
    }
}

/// The turn that ended last in a conversation: the message that started it.
const LAST_ENDED_TURN: &str = "
SELECT MAX(u.id) FROM events u
WHERE u.conversation_id = ?1 AND u.kind = 'user_text'
    AND EXISTS (
        SELECT 1 FROM events e
        WHERE e.conversation_id = ?1 AND e.id > u.id
            AND e.kind IN ('turn_ended', 'turn_failed', 'turn_interrupted')
    )
";

const USAGE_COLUMNS: &str = "provider, context_used, context_size, session_cost, turn_cost,
    cost_currency, input_tokens, output_tokens, total_tokens";

/// What a session cost grew by from `before` to `now`. A cost in another
/// unit, or one lower than before, belongs to a count that started again:
/// all of it is the turn's.
fn growth(now: &Cost, before: Option<&Cost>) -> Cost {
    let amount = match before {
        Some(before) if before.currency == now.currency && before.amount <= now.amount => {
            now.amount - before.amount
        }
        _ => now.amount,
    };
    Cost {
        amount,
        currency: now.currency.clone(),
    }
}

fn to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn usage_row(row: &Row<'_>) -> rusqlite::Result<TurnUsage> {
    let currency: Option<String> = row.get(5)?;
    let cost = |amount: Option<f64>| {
        amount
            .zip(currency.clone())
            .map(|(amount, currency)| Cost { amount, currency })
    };
    Ok(TurnUsage {
        provider: row.get(0)?,
        context_used: to_u64(row.get(1)?),
        context_size: to_u64(row.get(2)?),
        session_cost: cost(row.get(3)?),
        turn_cost: cost(row.get(4)?),
        input_tokens: to_u64(row.get(6)?),
        output_tokens: to_u64(row.get(7)?),
        total_tokens: to_u64(row.get(8)?),
    })
}

impl Store {
    /// Stores what the running turn of a conversation consumed, as the agent
    /// of `provider` reported it in the provider session `session_id`. The
    /// turn's cost is what the session's cost grew by since the last turn of
    /// the same session that reported one.
    pub fn record_usage(
        &self,
        conversation_id: i64,
        provider: &str,
        session_id: Option<&str>,
        reported: &Reported,
    ) -> Result<Option<TurnUsage>, StoreError> {
        if reported.is_empty() {
            return Ok(None);
        }
        let Some(turn) = self.connection.query_row(
            "SELECT MAX(id) FROM events WHERE conversation_id = ?1 AND kind = 'user_text'",
            params![conversation_id],
            |row| row.get::<_, Option<i64>>(0),
        )?
        else {
            return Ok(None);
        };
        let before = match &reported.session_cost {
            Some(_) => self
                .connection
                .query_row(
                    "SELECT session_cost, cost_currency FROM turn_usage
                     WHERE conversation_id = ?1 AND provider = ?2 AND session_id IS ?3
                        AND session_cost IS NOT NULL AND turn_event_id < ?4
                     ORDER BY turn_event_id DESC LIMIT 1",
                    params![conversation_id, provider, session_id, turn],
                    |row| {
                        Ok(Cost {
                            amount: row.get(0)?,
                            currency: row.get(1)?,
                        })
                    },
                )
                .optional()?,
            None => None,
        };
        let usage = TurnUsage {
            provider: provider.to_owned(),
            context_used: reported.context.map(|(used, _)| used),
            context_size: reported.context.map(|(_, size)| size),
            turn_cost: reported
                .session_cost
                .as_ref()
                .map(|now| growth(now, before.as_ref())),
            session_cost: reported.session_cost.clone(),
            input_tokens: reported.tokens.map(|tokens| tokens.input),
            output_tokens: reported.tokens.map(|tokens| tokens.output),
            total_tokens: reported.tokens.map(|tokens| tokens.total),
        };
        self.connection.execute(
            "INSERT OR REPLACE INTO turn_usage (turn_event_id, conversation_id, provider,
                session_id, context_used, context_size, session_cost, turn_cost, cost_currency,
                input_tokens, output_tokens, total_tokens, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                turn,
                conversation_id,
                provider,
                session_id,
                usage.context_used.map(to_i64),
                usage.context_size.map(to_i64),
                usage.session_cost.as_ref().map(|cost| cost.amount),
                usage.turn_cost.as_ref().map(|cost| cost.amount),
                usage
                    .session_cost
                    .as_ref()
                    .map(|cost| cost.currency.clone()),
                usage.input_tokens.map(to_i64),
                usage.output_tokens.map(to_i64),
                usage.total_tokens.map(to_i64),
                now_ms()
            ],
        )?;
        Ok(Some(usage))
    }

    /// What the last turn that ended in a conversation consumed. `None` when
    /// its agent reported nothing, or when no turn ended yet.
    pub fn last_usage(&self, conversation_id: i64) -> Result<Option<TurnUsage>, StoreError> {
        let turn: Option<i64> =
            self.connection
                .query_row(LAST_ENDED_TURN, params![conversation_id], |row| row.get(0))?;
        let Some(turn) = turn else {
            return Ok(None);
        };
        let usage = self
            .connection
            .query_row(
                &format!("SELECT {USAGE_COLUMNS} FROM turn_usage WHERE turn_event_id = ?1"),
                params![turn],
                usage_row,
            )
            .optional()?;
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{Event, TurnEnd};
    use rusqlite::Connection;

    use super::*;

    fn store() -> (Store, i64) {
        let store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        store
            .connection
            .execute_batch(
                "INSERT INTO conversations (provider, cwd, created_at_ms) VALUES ('claude', '/', 0)",
            )
            .unwrap();
        let id = store.connection.last_insert_rowid();
        (store, id)
    }

    fn usd(amount: f64) -> Cost {
        Cost {
            amount,
            currency: "USD".to_owned(),
        }
    }

    fn turn(store: &Store, id: i64, reported: &Reported) -> Option<TurnUsage> {
        store.append(id, &Event::user_text("go")).unwrap();
        let usage = store
            .record_usage(id, "claude", Some("s1"), reported)
            .unwrap();
        store
            .append(id, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
        usage
    }

    #[test]
    fn a_turn_s_cost_is_what_its_session_s_cost_grew_by() {
        let (store, id) = store();
        let first = turn(
            &store,
            id,
            &Reported {
                context: Some((100, 1000)),
                session_cost: Some(usd(0.5)),
                tokens: Some(Tokens {
                    input: 10,
                    output: 20,
                    total: 30,
                }),
            },
        )
        .unwrap();
        assert_eq!(first.turn_cost, Some(usd(0.5)));
        assert_eq!(first.total_tokens, Some(30));
        let second = turn(
            &store,
            id,
            &Reported {
                context: Some((300, 1000)),
                session_cost: Some(usd(1.25)),
                tokens: None,
            },
        )
        .unwrap();
        assert_eq!(second.turn_cost, Some(usd(0.75)));
        assert_eq!(second.input_tokens, None);
        assert_eq!(store.last_usage(id).unwrap(), Some(second));
    }

    #[test]
    fn a_turn_that_reported_nothing_has_no_row_and_no_usage() {
        let (store, id) = store();
        turn(
            &store,
            id,
            &Reported {
                context: Some((1, 2)),
                ..Reported::default()
            },
        );
        assert_eq!(turn(&store, id, &Reported::default()), None);
        assert_eq!(store.last_usage(id).unwrap(), None);
    }

    #[test]
    fn a_cost_that_fell_or_changed_unit_counts_whole() {
        assert_eq!(growth(&usd(0.25), Some(&usd(2.0))), usd(0.25));
        let credits = Cost {
            amount: 3.0,
            currency: "credits".to_owned(),
        };
        assert_eq!(growth(&credits, Some(&usd(1.0))), credits);
        assert_eq!(growth(&usd(1.0), None), usd(1.0));
    }
}
