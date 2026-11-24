//! Routines and their runs (EP-007). A routine owns a conversation, which
//! each run is a turn of. The scheduler moves a routine's next occurrence
//! and records the run in one transaction (`fire`), so an occurrence never
//! runs twice, whatever stops the daemon after it.

use crowbot_protocol::{
    HarnessSettings, Routine, RoutineIncident, RoutineRun, RoutineScript, RoutineState, RunOutcome,
    Schedule, TurnRefusal,
};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, now_ms};

/// A routine as the store keeps it: what clients see, and the retry it
/// waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineRow {
    pub routine: Routine,
    /// When the run that failed before its agent said anything is tried
    /// again (US-034).
    pub retry_at_ms: Option<i64>,
    /// The retries already made for the last occurrence.
    pub retry_attempt: u32,
}

/// A routine to store.
#[derive(Debug, Clone)]
pub struct NewRoutine {
    pub bot_id: i64,
    pub name: String,
    pub schedule: Schedule,
    pub prompt: String,
    pub skills: Vec<String>,
    pub skip_missed: bool,
    pub script: Option<RoutineScript>,
    pub state: RoutineState,
    pub next_ms: Option<i64>,
}

/// What the scheduler decided for a routine that came due, applied in one
/// transaction with the run it records.
#[derive(Debug, Clone)]
pub struct Fire {
    pub routine_id: i64,
    /// The occurrence the run is for: the stored next occurrence, or the
    /// time of a retry.
    pub due_ms: i64,
    pub at_ms: i64,
    /// The retry of the last occurrence this run is, or 0.
    pub attempt: u32,
    /// A retry leaves the next occurrence where it is.
    pub retry: bool,
    pub next_ms: Option<i64>,
    pub state: RoutineState,
    /// `Queued` for a run that starts, `Missed` for one that does not.
    pub outcome: RunOutcome,
    pub detail: Option<String>,
}

/// A run as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: i64,
    pub routine_id: i64,
    pub bot_id: i64,
    pub conversation_id: i64,
    pub due_ms: i64,
    pub attempt: u32,
    pub outcome: RunOutcome,
    pub silent: bool,
    /// The routine's prompt when the run was recorded.
    pub prompt: String,
}

/// How a run ended.
#[derive(Debug, Clone, Default)]
pub struct RunEnd {
    pub refusal: Option<TurnRefusal>,
    pub detail: Option<String>,
}

fn name_of<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn read_name<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

const ROUTINE_COLUMNS: &str = "id, bot_id, conversation_id, name, schedule, prompt, skills,
    skip_missed, script_path, script_hash, state, next_ms, retry_at_ms, retry_attempt, incident,
    created_at_ms";

/// A row of `ROUTINE_COLUMNS`, with its last run still to read.
fn routine_row(row: &Row<'_>) -> rusqlite::Result<(RoutineRow, String, String, Option<String>)> {
    let script_path: Option<String> = row.get(8)?;
    let script_hash: Option<String> = row.get(9)?;
    let state: String = row.get(10)?;
    let routine = Routine {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        conversation_id: row.get(2)?,
        name: row.get(3)?,
        schedule: Schedule::Every { seconds: 0 },
        prompt: row.get(5)?,
        skills: Vec::new(),
        skip_missed: row.get(7)?,
        script: script_path
            .zip(script_hash)
            .map(|(path, hash)| RoutineScript { path, hash }),
        state: read_name(&state).unwrap_or(RoutineState::Paused),
        next_ms: row.get(11)?,
        last_run: None,
        incident: None,
        created_at_ms: row.get(15)?,
    };
    Ok((
        RoutineRow {
            routine,
            retry_at_ms: row.get(12)?,
            retry_attempt: row.get(13)?,
        },
        row.get(4)?,
        row.get(6)?,
        row.get(14)?,
    ))
}

const RUN_COLUMNS: &str =
    "id, due_ms, at_ms, outcome, refusal, detail, silent, attempt, ended_at_ms";

fn run_of(row: &Row<'_>) -> rusqlite::Result<RoutineRun> {
    let outcome: String = row.get(3)?;
    let refusal: Option<String> = row.get(4)?;
    Ok(RoutineRun {
        id: row.get(0)?,
        due_ms: row.get(1)?,
        at_ms: row.get(2)?,
        outcome: read_name(&outcome).unwrap_or(RunOutcome::Other),
        refusal: refusal.as_deref().and_then(read_name),
        detail: row.get(5)?,
        silent: row.get(6)?,
        attempt: row.get(7)?,
        ended_at_ms: row.get(8)?,
    })
}

impl Store {
    fn finish_routine(
        &self,
        (mut row, schedule, skills, incident): (RoutineRow, String, String, Option<String>),
    ) -> Result<RoutineRow, StoreError> {
        row.routine.schedule = serde_json::from_str(&schedule)?;
        row.routine.skills = serde_json::from_str(&skills)?;
        row.routine.incident = incident
            .as_deref()
            .map(serde_json::from_str::<RoutineIncident>)
            .transpose()?;
        row.routine.last_run = self.last_run(row.routine.id)?;
        Ok(row)
    }

    fn routines_where(
        &self,
        condition: &str,
        parameters: impl rusqlite::Params,
    ) -> Result<Vec<RoutineRow>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {ROUTINE_COLUMNS} FROM routines WHERE {condition} ORDER BY id"
        ))?;
        let rows = statement
            .query_map(parameters, routine_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|row| self.finish_routine(row))
            .collect()
    }

    /// The routines of a bot, the oldest first.
    pub fn routines(&self, bot_id: i64) -> Result<Vec<Routine>, StoreError> {
        Ok(self
            .routines_where("bot_id = ?1", params![bot_id])?
            .into_iter()
            .map(|row| row.routine)
            .collect())
    }

    pub fn routine(&self, routine_id: i64) -> Result<Option<RoutineRow>, StoreError> {
        Ok(self
            .routines_where("id = ?1", params![routine_id])?
            .into_iter()
            .next())
    }

    /// The routine a conversation belongs to, when it is one's.
    pub fn routine_of_conversation(&self, conversation_id: i64) -> Result<Option<i64>, StoreError> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM routines WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Stores a routine with a conversation of its own, which is not the
    /// bot's current one, titled with the routine's name. With
    /// `switch_on`, the bot's `routine` switch is turned on in the same
    /// transaction. `None` means the bot does not exist.
    pub fn create_routine(
        &self,
        new: &NewRoutine,
        switch_on: bool,
    ) -> Result<Option<i64>, StoreError> {
        self.atomically(|| self.insert_routine(new, switch_on))
    }

    fn insert_routine(&self, new: &NewRoutine, switch_on: bool) -> Result<Option<i64>, StoreError> {
        let transaction = &self.connection;
        let now = now_ms();
        let inserted = transaction.execute(
            "INSERT INTO conversations (bot_id, provider, cwd, title, created_at_ms)
             SELECT id, provider, workspace, ?2, ?3 FROM bots WHERE id = ?1",
            params![new.bot_id, new.name, now],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let conversation_id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO routines (bot_id, conversation_id, name, schedule, prompt, skills,
                skip_missed, script_path, script_hash, state, next_ms, created_at_ms,
                updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                new.bot_id,
                conversation_id,
                new.name,
                serde_json::to_string(&new.schedule)?,
                new.prompt,
                serde_json::to_string(&new.skills)?,
                new.skip_missed,
                new.script.as_ref().map(|script| &script.path),
                new.script.as_ref().map(|script| &script.hash),
                name_of(new.state),
                new.next_ms,
                now
            ],
        )?;
        let routine_id = transaction.last_insert_rowid();
        if switch_on {
            self.turn_routine_switch_on(new.bot_id)?;
        }
        Ok(Some(routine_id))
    }

    /// Runs `change` all or none, inside a transaction that may already be
    /// open (the one a `Mutation` applies in) or on its own.
    fn atomically<T>(
        &self,
        change: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.connection.execute_batch("SAVEPOINT routine_change")?;
        match change() {
            Ok(done) => {
                self.connection.execute_batch("RELEASE routine_change")?;
                Ok(done)
            }
            Err(error) => {
                self.connection
                    .execute_batch("ROLLBACK TO routine_change; RELEASE routine_change")?;
                Err(error)
            }
        }
    }

    /// Turns the bot's `routine` switch on, as the act of a person who
    /// created or approved a routine (decision D4).
    pub fn turn_routine_switch_on(&self, bot_id: i64) -> Result<(), StoreError> {
        let mut settings: HarnessSettings = self.harness_settings(bot_id)?;
        if !settings.switches.routine {
            settings.switches.routine = true;
            self.set_harness_settings(bot_id, &settings)?;
        }
        Ok(())
    }

    /// Replaces what a routine does and when, as approved by a person. The
    /// switch is turned on as `create_routine` does.
    pub fn change_routine(
        &self,
        routine_id: i64,
        new: &NewRoutine,
        switch_on: bool,
    ) -> Result<bool, StoreError> {
        self.atomically(|| self.replace_routine(routine_id, new, switch_on))
    }

    fn replace_routine(
        &self,
        routine_id: i64,
        new: &NewRoutine,
        switch_on: bool,
    ) -> Result<bool, StoreError> {
        let transaction = &self.connection;
        let changed = transaction.execute(
            "UPDATE routines SET name = ?2, schedule = ?3, prompt = ?4, skills = ?5,
                skip_missed = ?6, script_path = ?7, script_hash = ?8, state = ?9, next_ms = ?10,
                retry_at_ms = NULL, retry_attempt = 0, updated_at_ms = ?11
             WHERE id = ?1 AND bot_id = ?12",
            params![
                routine_id,
                new.name,
                serde_json::to_string(&new.schedule)?,
                new.prompt,
                serde_json::to_string(&new.skills)?,
                new.skip_missed,
                new.script.as_ref().map(|script| &script.path),
                new.script.as_ref().map(|script| &script.hash),
                name_of(new.state),
                new.next_ms,
                now_ms(),
                new.bot_id
            ],
        )?;
        if changed > 0 {
            transaction.execute(
                "UPDATE conversations SET title = ?2
                 WHERE id = (SELECT conversation_id FROM routines WHERE id = ?1)",
                params![routine_id, new.name],
            )?;
            if switch_on {
                self.turn_routine_switch_on(new.bot_id)?;
            }
        }
        Ok(changed > 0)
    }

    /// Sets the state and the next occurrence of a routine. A routine that
    /// leaves `Active` drops the retry it waited for.
    pub fn set_routine_state(
        &self,
        routine_id: i64,
        state: RoutineState,
        next_ms: Option<i64>,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE routines SET state = ?2, next_ms = ?3, updated_at_ms = ?4,
                retry_at_ms = CASE WHEN ?2 = 'active' THEN retry_at_ms END
             WHERE id = ?1",
            params![routine_id, name_of(state), next_ms, now_ms()],
        )?;
        Ok(changed > 0)
    }

    /// Removes a routine and its runs. Its conversation stays, as a
    /// conversation of the bot. `None` when there was no such routine.
    pub fn delete_routine(&self, routine_id: i64) -> Result<Option<i64>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let bot_id: Option<i64> = transaction
            .query_row(
                "SELECT bot_id FROM routines WHERE id = ?1",
                params![routine_id],
                |row| row.get(0),
            )
            .optional()?;
        transaction.execute(
            "DELETE FROM routine_runs WHERE routine_id = ?1",
            params![routine_id],
        )?;
        transaction.execute("DELETE FROM routines WHERE id = ?1", params![routine_id])?;
        transaction.commit()?;
        Ok(bot_id)
    }

    /// The active routines whose next occurrence, or whose retry, is due at
    /// `now`.
    pub fn due_routines(&self, now: i64) -> Result<Vec<RoutineRow>, StoreError> {
        self.routines_where(
            "state = 'active' AND (next_ms <= ?1 OR retry_at_ms <= ?1)",
            params![now],
        )
    }

    /// The soonest occurrence or retry of an active routine.
    pub fn next_due(&self) -> Result<Option<i64>, StoreError> {
        let next = self.connection.query_row(
            "SELECT MIN(MIN(COALESCE(next_ms, 9223372036854775807),
                COALESCE(retry_at_ms, 9223372036854775807)))
             FROM routines WHERE state = 'active'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(next.filter(|next| *next != i64::MAX))
    }

    /// Applies what the scheduler decided and records the run, in one
    /// transaction. A run to queue while the routine's last run has not
    /// ended is recorded as skipped. `None` when the routine changed since
    /// it was read: another decision already applied, or a person paused it.
    pub fn fire(&self, fire: &Fire) -> Result<Option<(i64, RunOutcome)>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let current: Option<(Option<i64>, Option<i64>, String, String)> = transaction
            .query_row(
                "SELECT next_ms, retry_at_ms, state, prompt FROM routines WHERE id = ?1",
                params![fire.routine_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((next_ms, retry_at_ms, state, prompt)) = current else {
            return Ok(None);
        };
        let expected = if fire.retry { retry_at_ms } else { next_ms };
        if state != "active" || expected != Some(fire.due_ms) {
            return Ok(None);
        }
        let mut outcome = fire.outcome;
        let mut detail = fire.detail.clone();
        if outcome == RunOutcome::Queued && self.unfinished_run(fire.routine_id)? {
            outcome = RunOutcome::Skipped;
            detail = Some("The previous run had not ended.".to_owned());
        }
        if fire.retry {
            transaction.execute(
                "UPDATE routines SET retry_at_ms = NULL WHERE id = ?1",
                params![fire.routine_id],
            )?;
        } else {
            transaction.execute(
                "UPDATE routines SET next_ms = ?2, state = ?3, updated_at_ms = ?4,
                    retry_at_ms = CASE WHEN ?5 THEN NULL ELSE retry_at_ms END,
                    retry_attempt = CASE WHEN ?5 THEN 0 ELSE retry_attempt END
                 WHERE id = ?1",
                params![
                    fire.routine_id,
                    fire.next_ms,
                    name_of(fire.state),
                    now_ms(),
                    outcome == RunOutcome::Queued
                ],
            )?;
        }
        let ended = (!outcome.unfinished()).then_some(fire.at_ms);
        transaction.execute(
            "INSERT INTO routine_runs (routine_id, due_ms, at_ms, attempt, outcome, detail,
                prompt, ended_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                fire.routine_id,
                fire.due_ms,
                fire.at_ms,
                fire.attempt,
                name_of(outcome),
                detail,
                prompt,
                ended
            ],
        )?;
        let run_id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(Some((run_id, outcome)))
    }

    /// Records a run a person asked for, which leaves the next occurrence
    /// where it is. `None` when the routine does not exist; a run that
    /// cannot start because the last one has not ended is recorded skipped.
    pub fn run_now(
        &self,
        routine_id: i64,
        at_ms: i64,
    ) -> Result<Option<(i64, RunOutcome)>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let prompt: Option<String> = transaction
            .query_row(
                "SELECT prompt FROM routines WHERE id = ?1",
                params![routine_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(prompt) = prompt else {
            return Ok(None);
        };
        let (outcome, detail) = if self.unfinished_run(routine_id)? {
            (
                RunOutcome::Skipped,
                Some("The previous run had not ended.".to_owned()),
            )
        } else {
            (RunOutcome::Queued, None)
        };
        let ended = (!outcome.unfinished()).then_some(at_ms);
        transaction.execute(
            "INSERT INTO routine_runs (routine_id, due_ms, at_ms, outcome, detail, prompt,
                ended_at_ms)
             VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)",
            params![routine_id, at_ms, name_of(outcome), detail, prompt, ended],
        )?;
        let run_id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(Some((run_id, outcome)))
    }

    fn unfinished_run(&self, routine_id: i64) -> Result<bool, StoreError> {
        let unfinished = self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM routine_runs
                WHERE routine_id = ?1 AND outcome IN ('queued', 'running'))",
            params![routine_id],
            |row| row.get(0),
        )?;
        Ok(unfinished)
    }

    pub fn last_run(&self, routine_id: i64) -> Result<Option<RoutineRun>, StoreError> {
        let run = self
            .connection
            .query_row(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM routine_runs WHERE routine_id = ?1
                     ORDER BY id DESC LIMIT 1"
                ),
                params![routine_id],
                run_of,
            )
            .optional()?;
        Ok(run)
    }

    pub fn run(&self, run_id: i64) -> Result<Option<RunRow>, StoreError> {
        let run = self
            .connection
            .query_row(
                "SELECT r.id, r.routine_id, t.bot_id, t.conversation_id, r.due_ms, r.attempt,
                    r.outcome, r.silent, r.prompt
                 FROM routine_runs r JOIN routines t ON t.id = r.routine_id WHERE r.id = ?1",
                params![run_id],
                |row| {
                    let outcome: String = row.get(6)?;
                    Ok(RunRow {
                        id: row.get(0)?,
                        routine_id: row.get(1)?,
                        bot_id: row.get(2)?,
                        conversation_id: row.get(3)?,
                        due_ms: row.get(4)?,
                        attempt: row.get(5)?,
                        outcome: read_name(&outcome).unwrap_or(RunOutcome::Other),
                        silent: row.get(7)?,
                        prompt: row.get(8)?,
                    })
                },
            )
            .optional()?;
        Ok(run)
    }

    /// A queued run starts: its turn is recorded next.
    pub fn run_started(&self, run_id: i64) -> Result<bool, StoreError> {
        let started = self.connection.execute(
            "UPDATE routine_runs SET outcome = 'running' WHERE id = ?1 AND outcome = 'queued'",
            params![run_id],
        )?;
        Ok(started > 0)
    }

    /// Records how a run that has not ended yet ended. `false` when it had
    /// already ended.
    pub fn end_run(
        &self,
        run_id: i64,
        outcome: RunOutcome,
        end: &RunEnd,
        at_ms: i64,
    ) -> Result<bool, StoreError> {
        let ended = self.connection.execute(
            "UPDATE routine_runs SET outcome = ?2, refusal = ?3, detail = COALESCE(?4, detail),
                ended_at_ms = ?5
             WHERE id = ?1 AND outcome IN ('queued', 'running')",
            params![
                run_id,
                name_of(outcome),
                end.refusal.map(name_of),
                end.detail,
                at_ms
            ],
        )?;
        Ok(ended > 0)
    }

    /// Records what the running run of a routine's conversation said of
    /// itself (`routine_result`). `false` when no run of it is running.
    pub fn set_run_silent(&self, conversation_id: i64, silent: bool) -> Result<bool, StoreError> {
        let set = self.connection.execute(
            "UPDATE routine_runs SET silent = ?2
             WHERE outcome = 'running'
                AND routine_id = (SELECT id FROM routines WHERE conversation_id = ?1)",
            params![conversation_id, silent],
        )?;
        Ok(set > 0)
    }

    /// Whether a turn of the conversation that ended before its last one
    /// is still unread.
    pub fn unread_before_last_turn(&self, conversation_id: i64) -> Result<bool, StoreError> {
        let unread = self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM events e JOIN conversations c ON c.id = e.conversation_id
                WHERE c.id = ?1 AND e.id > c.read_event_id
                    AND e.kind IN ('turn_ended', 'turn_failed', 'turn_interrupted')
                    AND e.id < (SELECT MAX(id) FROM events
                        WHERE conversation_id = ?1 AND kind = 'user_text'))",
            params![conversation_id],
            |row| row.get(0),
        )?;
        Ok(unread)
    }

    /// Waits a retry of the last occurrence until `at_ms`, the `attempt`th.
    pub fn set_retry(&self, routine_id: i64, at_ms: i64, attempt: u32) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE routines SET retry_at_ms = ?2, retry_attempt = ?3 WHERE id = ?1",
            params![routine_id, at_ms, attempt],
        )?;
        Ok(())
    }

    pub fn set_incident(
        &self,
        routine_id: i64,
        incident: Option<&RoutineIncident>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE routines SET incident = ?2 WHERE id = ?1",
            params![routine_id, incident.map(serde_json::to_string).transpose()?],
        )?;
        Ok(())
    }

    /// A run succeeded: the failure it repeated and the retries are over.
    pub fn routine_succeeded(&self, routine_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE routines SET incident = NULL, retry_attempt = 0, retry_at_ms = NULL
             WHERE id = ?1",
            params![routine_id],
        )?;
        Ok(())
    }

    /// Ends every run that a previous daemon recorded and did not end: its
    /// occurrence has moved on, so it never runs again.
    pub(super) fn interrupt_runs(&self) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE routine_runs SET outcome = 'interrupted', ended_at_ms = ?1,
                detail = 'The daemon stopped before the run ended.'
             WHERE outcome IN ('queued', 'running')",
            params![now_ms()],
        )?;
        Ok(())
    }

    /// The names of the skills the routines of a bot load.
    pub fn routine_skill_names(&self, bot_id: i64) -> Result<Vec<String>, StoreError> {
        let mut names: Vec<String> = Vec::new();
        for routine in self.routines(bot_id)? {
            names.extend(routine.skills);
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Removes the routines of a bot that is deleted, and their runs.
    pub(super) fn forget_routines(&self, bot_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM routine_runs WHERE routine_id IN
                (SELECT id FROM routines WHERE bot_id = ?1)",
            params![bot_id],
        )?;
        self.connection
            .execute("DELETE FROM routines WHERE bot_id = ?1", params![bot_id])?;
        Ok(())
    }

    /// Gives each routine of a bot whose history is deleted a new, empty
    /// conversation, and forgets its runs. Called inside the transaction
    /// that removes the conversations, before it removes them.
    pub(super) fn rehome_routines(&self, bot_id: i64) -> Result<Vec<i64>, StoreError> {
        let routines: Vec<(i64, String)> = {
            let mut statement = self
                .connection
                .prepare("SELECT id, name FROM routines WHERE bot_id = ?1")?;
            statement
                .query_map(params![bot_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut kept = Vec::new();
        for (routine_id, name) in routines {
            self.connection.execute(
                "INSERT INTO conversations (bot_id, provider, cwd, title, created_at_ms)
                 SELECT id, provider, workspace, ?2, ?3 FROM bots WHERE id = ?1",
                params![bot_id, name, now_ms()],
            )?;
            let conversation_id = self.connection.last_insert_rowid();
            self.connection.execute(
                "UPDATE routines SET conversation_id = ?2 WHERE id = ?1",
                params![routine_id, conversation_id],
            )?;
            self.connection.execute(
                "DELETE FROM routine_runs WHERE routine_id = ?1",
                params![routine_id],
            )?;
            kept.push(conversation_id);
        }
        Ok(kept)
    }

    /// The time a pass of the daemon named `name` last ran.
    pub fn mark(&self, name: &str) -> Result<Option<i64>, StoreError> {
        let at = self
            .connection
            .query_row(
                "SELECT at_ms FROM daemon_marks WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(at)
    }

    pub fn set_mark(&self, name: &str, at_ms: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO daemon_marks (name, at_ms) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET at_ms = excluded.at_ms",
            params![name, at_ms],
        )?;
        Ok(())
    }

    /// Forgets the provider sessions of a conversation, so that its next
    /// turn starts a new one: each run of a routine starts clean.
    pub fn forget_provider_sessions(&self, conversation_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM provider_sessions WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::{Avatar, MascotShape, PermissionLevel};

    use crate::store::NewBot;

    fn store() -> (Store, i64) {
        let store =
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let bot = store
            .create_bot(&NewBot {
                name: "Ada".to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#3b82f6".to_owned(),
                    shape: MascotShape::Smooth,
                },
                provider: "custom".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/".to_owned(),
            })
            .unwrap();
        (store, bot.bot.id)
    }

    fn new(bot_id: i64, next_ms: i64) -> NewRoutine {
        NewRoutine {
            bot_id,
            name: "Morning".to_owned(),
            schedule: Schedule::Every { seconds: 3600 },
            prompt: "say hi".to_owned(),
            skills: vec!["deploy".to_owned()],
            skip_missed: false,
            script: None,
            state: RoutineState::Active,
            next_ms: Some(next_ms),
        }
    }

    fn fire(routine_id: i64, due_ms: i64, next_ms: i64) -> Fire {
        Fire {
            routine_id,
            due_ms,
            at_ms: due_ms,
            attempt: 0,
            retry: false,
            next_ms: Some(next_ms),
            state: RoutineState::Active,
            outcome: RunOutcome::Queued,
            detail: None,
        }
    }

    #[test]
    fn a_routine_holds_its_bot_schedule_prompt_skills_state_and_next_occurrence() {
        let (store, bot) = store();
        let id = store
            .create_routine(&new(bot, 1_000), true)
            .unwrap()
            .unwrap();
        let row = store.routine(id).unwrap().unwrap();
        let routine = row.routine;
        assert_eq!(routine.bot_id, bot);
        assert_eq!(routine.schedule, Schedule::Every { seconds: 3600 });
        assert_eq!(routine.prompt, "say hi");
        assert_eq!(routine.skills, ["deploy"]);
        assert_eq!(routine.state, RoutineState::Active);
        assert_eq!(routine.next_ms, Some(1_000));
        assert_eq!(
            store
                .routine_of_conversation(routine.conversation_id)
                .unwrap(),
            Some(id)
        );
        assert!(store.harness_settings(bot).unwrap().switches.routine);
    }

    #[test]
    fn an_occurrence_fires_once_and_a_run_that_has_not_ended_skips_the_next() {
        let (store, bot) = store();
        let id = store
            .create_routine(&new(bot, 1_000), false)
            .unwrap()
            .unwrap();
        let (run, outcome) = store.fire(&fire(id, 1_000, 2_000)).unwrap().unwrap();
        assert_eq!(outcome, RunOutcome::Queued);
        // The same occurrence again finds the next one moved: nothing.
        assert_eq!(store.fire(&fire(id, 1_000, 2_000)).unwrap(), None);
        assert_eq!(
            store.routine(id).unwrap().unwrap().routine.next_ms,
            Some(2_000)
        );
        assert!(store.run_started(run).unwrap());
        let (_, next) = store.fire(&fire(id, 2_000, 3_000)).unwrap().unwrap();
        assert_eq!(next, RunOutcome::Skipped);
        assert!(
            store
                .end_run(run, RunOutcome::Completed, &RunEnd::default(), 2_500)
                .unwrap()
        );
        assert!(
            !store
                .end_run(run, RunOutcome::Failed, &RunEnd::default(), 2_600)
                .unwrap()
        );
        let (_, third) = store.fire(&fire(id, 3_000, 4_000)).unwrap().unwrap();
        assert_eq!(third, RunOutcome::Queued);
    }

    #[test]
    fn a_deleted_routine_s_ids_are_never_given_again() {
        let (store, bot) = store();
        let first = store
            .create_routine(&new(bot, 1_000), false)
            .unwrap()
            .unwrap();
        let (run, _) = store.fire(&fire(first, 1_000, 2_000)).unwrap().unwrap();
        store.delete_routine(first).unwrap();
        let second = store
            .create_routine(&new(bot, 1_000), false)
            .unwrap()
            .unwrap();
        assert!(second > first);
        let (later, _) = store.fire(&fire(second, 1_000, 2_000)).unwrap().unwrap();
        assert!(later > run);
        // What a script of the deleted run still asks changes nothing.
        assert!(
            !store
                .end_run(run, RunOutcome::Silent, &RunEnd::default(), 1_500)
                .unwrap()
        );
        assert!(store.run(run).unwrap().is_none());
    }

    #[test]
    fn a_restart_ends_the_runs_a_daemon_left_and_they_never_run_again() {
        let (store, bot) = store();
        let id = store
            .create_routine(&new(bot, 1_000), false)
            .unwrap()
            .unwrap();
        let (run, _) = store.fire(&fire(id, 1_000, 2_000)).unwrap().unwrap();
        store.recover().unwrap();
        assert_eq!(
            store.run(run).unwrap().unwrap().outcome,
            RunOutcome::Interrupted
        );
        assert!(!store.run_started(run).unwrap());
    }
}
