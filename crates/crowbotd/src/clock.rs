//! The time routines and the curator are scheduled by. It is the system's
//! clock, except in a debug build started with `CROWBOT_TEST_CLOCK`, which
//! names a file that holds the time in milliseconds since the Unix epoch:
//! the daemon's tests move it to make a routine due without waiting.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the scheduler looks at the clock when nothing is due sooner. A
/// clock that a person or the system moves is seen within this time.
const TICK: Duration = Duration::from_secs(30);

pub fn now_ms() -> i64 {
    #[cfg(debug_assertions)]
    if let Some(now) = controlled::now() {
        return now;
    }
    system_ms()
}

fn system_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// The longest the scheduler sleeps before it looks at the clock again.
pub fn tick() -> Duration {
    #[cfg(debug_assertions)]
    if controlled::file().is_some() {
        return Duration::from_millis(50);
    }
    TICK
}

#[cfg(debug_assertions)]
mod controlled {
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI64, Ordering};

    const VARIABLE: &str = "CROWBOT_TEST_CLOCK";

    pub fn file() -> Option<&'static PathBuf> {
        static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
        FILE.get_or_init(|| std::env::var_os(VARIABLE).map(PathBuf::from))
            .as_ref()
    }

    /// The time the file holds. A file being written, which does not read
    /// as a number, gives the time read last: the clock never jumps back to
    /// the system's.
    pub fn now() -> Option<i64> {
        static LAST: AtomicI64 = AtomicI64::new(i64::MIN);
        let file = file()?;
        let read = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok());
        match read {
            Some(now) => {
                LAST.store(now, Ordering::SeqCst);
                Some(now)
            }
            None => match LAST.load(Ordering::SeqCst) {
                i64::MIN => Some(super::system_ms()),
                last => Some(last),
            },
        }
    }
}
