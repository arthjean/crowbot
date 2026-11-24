//! The figures of the curator. The daemon's tests read this file too, so
//! that a test and the code share each figure.

use std::time::Duration;

const DAY: u64 = 24 * 3600;

/// A skill unused this long is marked stale (`curator.py:43`,
/// `hermes-agent/agent/curator.py:42-46`).
pub const CURATOR_STALE: Duration = Duration::from_secs(14 * DAY);
/// A skill unused this long is archived (`curator.py:43`).
pub const CURATOR_ARCHIVE: Duration = Duration::from_secs(30 * DAY);
/// The time between two passes (`DEFAULT_INTERVAL_HOURS`, `curator.py:42`).
pub const CURATOR_INTERVAL: Duration = Duration::from_secs(7 * DAY);
/// How long no turn may have run before a pass (`DEFAULT_MIN_IDLE_HOURS`,
/// `curator.py:42`).
pub const CURATOR_IDLE: Duration = Duration::from_secs(2 * 3600);
