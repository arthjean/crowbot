//! Routines: a task a bot does on a schedule, in a conversation of its own,
//! whether or not a client is attached (EP-007 of the harness PRD).

use serde::{Deserialize, Serialize};

use crate::event::FailureCause;
use crate::harness::TurnRefusal;

/// The longest name of a routine, in characters.
pub const ROUTINE_NAME_CHARACTERS: usize = 80;
/// The longest prompt of a routine, in characters.
pub const ROUTINE_PROMPT_CHARACTERS: usize = 20_000;
/// The most skills a routine names.
pub const ROUTINE_SKILLS: usize = 8;
/// The shortest period of an `Every` schedule, in seconds.
pub const ROUTINE_EVERY_MIN_SECONDS: u64 = 60;
/// The longest period of an `Every` schedule, in seconds: a year and a day.
pub const ROUTINE_EVERY_MAX_SECONDS: u64 = 366 * 24 * 3600;

/// When a routine is due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Once, at this instant, in milliseconds since the Unix epoch.
    Once { at_ms: i64 },
    /// Every `seconds`, counted in UTC from the last occurrence, so that a
    /// change of the clock's offset does not move it.
    Every { seconds: u64 },
    /// The five fields of a cron expression (minute, hour, day of the month,
    /// month, day of the week), read on the wall clock of `zone`, an IANA
    /// time zone name such as `Europe/Paris`.
    Cron { expression: String, zone: String },
}

/// Where a routine stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineState {
    /// Due at its next occurrence.
    Active,
    /// Not due until a person resumes it.
    Paused,
    /// A `Once` routine that ran: never due again.
    Finished,
    /// A `Once` routine whose instant passed while the daemon could not run
    /// it: never due again.
    Missed,
}

/// The guard script of a routine: a program of the bot's folder that runs
/// before each run and decides whether the bot is needed. `hash` is the
/// SHA-256 of its content when a person approved it; a script whose content
/// changed since does not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineScript {
    pub path: String,
    pub hash: String,
}

/// A routine as a person writes it in a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineDraft {
    pub name: String,
    pub schedule: Schedule,
    pub prompt: String,
    /// The skills of the bot the run loads before it starts.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Occurrences missed while the daemon was down are not caught up.
    #[serde(default)]
    pub skip_missed: bool,
    /// The path of a guard script inside the bot's folder.
    #[serde(default)]
    pub script: Option<String>,
}

/// How one run of a routine went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Recorded and waiting to start.
    Queued,
    /// Its turn runs.
    Running,
    Completed,
    Failed,
    /// A guard of the daemon, or its restart, ended it.
    Interrupted,
    /// A person stopped it.
    Stopped,
    /// The previous run had not ended, or a turn of its conversation ran: this
    /// occurrence did not start.
    Skipped,
    /// The daemon did not start it: see `refusal`, or `detail`.
    NotStarted,
    /// It came due while the daemon could not run it, and was not caught up.
    Missed,
    /// The guard script found nothing new: no turn started.
    Silent,
    /// The guard script's output holds text the scan flags: no turn started.
    Blocked,
    /// The guard script failed, or ran too long: no turn started.
    ScriptFailed,
    #[serde(other)]
    Other,
}

impl RunOutcome {
    /// Whether the run still holds its routine: the next occurrence of a
    /// routine whose run has not ended is skipped.
    pub fn unfinished(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// One run of a routine: an occurrence that came due, or a retry of one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRun {
    pub id: i64,
    /// The occurrence the run is for.
    pub due_ms: i64,
    /// When the daemon recorded it.
    pub at_ms: i64,
    pub outcome: RunOutcome,
    /// Why the daemon did not start it.
    #[serde(default)]
    pub refusal: Option<TurnRefusal>,
    /// What a person reads of an outcome: the category a scan found, why a
    /// script failed.
    #[serde(default)]
    pub detail: Option<String>,
    /// The run said it had nothing to tell (`routine_result`).
    #[serde(default)]
    pub silent: bool,
    /// 0 for an occurrence, then 1, 2, 3 for its retries.
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
}

/// A failure that repeats: told once, then counted until a run succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineIncident {
    pub cause: FailureCause,
    pub since_ms: i64,
    /// The runs that failed with this cause since.
    pub runs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    pub id: i64,
    pub bot_id: i64,
    /// The conversation each run is a turn of.
    pub conversation_id: i64,
    pub name: String,
    pub schedule: Schedule,
    pub prompt: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub skip_missed: bool,
    #[serde(default)]
    pub script: Option<RoutineScript>,
    pub state: RoutineState,
    /// The next occurrence, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub next_ms: Option<i64>,
    /// The last run, with its end.
    #[serde(default)]
    pub last_run: Option<RoutineRun>,
    #[serde(default)]
    pub incident: Option<RoutineIncident>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineStateChange {
    Pause,
    Resume,
}

/// What a change did to a routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineChangeKind {
    Created,
    Changed,
    Paused,
    Resumed,
    Deleted,
    #[serde(other)]
    Other,
}

/// Why the daemon paused a routine on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutinePause {
    /// A run failed because the provider needs a sign-in.
    SignInRequired,
    /// The guard script changed since a person approved it.
    ScriptChanged,
    /// The bot has no daily limit of turns, which a routine needs.
    NoDailyLimit,
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_has_a_stable_stored_shape() {
        let cron = Schedule::Cron {
            expression: "0 9 * * 1-5".to_owned(),
            zone: "Europe/Paris".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&cron).unwrap(),
            r#"{"kind":"cron","expression":"0 9 * * 1-5","zone":"Europe/Paris"}"#
        );
        assert_eq!(
            serde_json::to_string(&Schedule::Every { seconds: 3600 }).unwrap(),
            r#"{"kind":"every","seconds":3600}"#
        );
        let once: Schedule = serde_json::from_str(r#"{"kind":"once","at_ms":5}"#).unwrap();
        assert_eq!(once, Schedule::Once { at_ms: 5 });
    }

    #[test]
    fn an_outcome_a_later_daemon_names_reads_as_other() {
        let read: RunOutcome = serde_json::from_str(r#""deferred""#).unwrap();
        assert_eq!(read, RunOutcome::Other);
        assert!(RunOutcome::Queued.unfinished() && RunOutcome::Running.unfinished());
        assert!(!RunOutcome::Skipped.unfinished());
    }
}
