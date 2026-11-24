//! The limits of the daemon's guards. The daemon's tests read this file too,
//! so that a test and the code share each figure.

use std::time::Duration;

/// The identical calls in a row, or the laps of an identical cycle of calls,
/// that stop a turn (`no_progress_block_after`,
/// `hermes-agent/agent/tool_guardrails.py:121`).
pub const NO_PROGRESS_REPEATS: usize = 5;
/// The longest cycle of calls the guard looks for
/// (`_STALL_GUARD_MAX_CYCLE_PERIOD`, `tool_guardrails.py:36-37`).
pub const NO_PROGRESS_CYCLE: usize = 4;
/// How long a turn the daemon started may receive nothing from its agent
/// (`_cron_inactivity_seconds`, `hermes-agent/cron/scheduler.py:1196`).
pub const INACTIVITY_LIMIT: Duration = Duration::from_secs(600);
/// The wait before each retry of a turn that failed before it said anything
/// (`hermes-agent/agent/turn_recovery_autorecover.py:23-24`); there are as
/// many retries as waits (`hermes-agent/agent/agent_init.py:1275`).
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
