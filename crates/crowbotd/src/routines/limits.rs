//! The figures of routines. The daemon's tests read this file too, so that
//! a test and the code share each figure.

use std::time::Duration;

/// How late a `Once` routine may start and still run
/// (`ONESHOT_GRACE_SECONDS`, `hermes-agent/cron/jobs.py:124`).
pub const ONCE_GRACE: Duration = Duration::from_secs(120);
/// How late a repeating routine may start and still run its occurrence:
/// half its period, within these bounds (`_MIN_GRACE_SECONDS` and
/// `_MAX_GRACE_SECONDS`, `hermes-agent/cron/jobs.py:947-964`).
pub const GRACE_MIN: Duration = Duration::from_secs(120);
pub const GRACE_MAX: Duration = Duration::from_secs(2 * 3600);
/// The most missed occurrences the daemon counts for one routine
/// (`_MAX_COUNTED_SLOTS`, `hermes-agent/cron/missed_runs.py:36`).
pub const MISSED_COUNT_MAX: u32 = 1_000;
/// The waits before each retry of a run that failed before its agent said
/// anything, and no further (`RETRY_DELAYS_SECONDS`,
/// `hermes-agent/cron/unreachable_retry.py:33`).
pub const RUN_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];
/// How long a guard script may run before the daemon ends it
/// (`hermes-agent/cron/scheduler.py:1384`). Debug builds divide it by
/// `CROWBOT_TEST_TIME_SCALE`.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(3_600);
/// The most bytes of a guard script's output the daemon reads.
pub const SCRIPT_OUTPUT_BYTES: usize = 64 << 10;
/// How every run's prompt opens.
pub const ROUTINE_OPENING: &str = "This is a run of a routine of Crow Bot.";
/// The line after which a run's prompt holds the routine's own prompt.
pub const ROUTINE_TASK: &str = "The task of the routine:";
