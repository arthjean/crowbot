//! One daemon per database. Before it opens the database, the daemon locks the
//! file `<db>.lock` for its whole life; the system releases the lock when the
//! daemon ends, however it ends. A second daemon on the same database, on any
//! endpoint, finds it locked and exits before it reads anything: it never
//! runs `Store::recover`, which would end the turns of the first one.
//!
//! The lock is a file of its own, not the database: SQLite's own locks on
//! Unix are lost when any other descriptor of the database closes, and the
//! Windows lock is mandatory, so SQLite could no longer read a locked file.
//! The lock file is never removed: a daemon that removed it would let the
//! next one lock a new file while a third still holds the old one.
//!
//! The file `<db>.owner` says which daemon holds the lock, for the message of
//! the one that finds it held. It is a file apart because the Windows lock
//! keeps every other process from reading the locked file. Its process id is
//! only ever shown: the lock, not that file, tells whether a daemon runs.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::Endpoint;
use serde::{Deserialize, Serialize};

/// How long a daemon waits for a lock that another one holds. Windows can
/// release the lock of a killed process a moment after it ended, and the
/// client starts a daemon again right after a crash.
const PATIENCE: Duration = Duration::from_secs(2);
const INTERVAL: Duration = Duration::from_millis(50);

/// The lock a daemon holds on its database for as long as it runs.
pub struct DatabaseLock {
    _file: File,
}

/// Why the database could not be locked.
pub enum LockFailure {
    /// Another daemon holds it; the text names that daemon when it can.
    Held(String),
    Failed(String),
}

#[derive(Serialize, Deserialize)]
struct Owner {
    pid: u32,
    endpoint: String,
    version: String,
    started_at: u64,
}

/// A file beside the database, named after it.
fn beside(db: &Path, extension: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(extension);
    db.with_file_name(name)
}

/// Locks the database for this daemon, and records which daemon holds it.
pub async fn lock_database(db: &Path, endpoint: &Endpoint) -> Result<DatabaseLock, LockFailure> {
    let path = beside(db, "lock");
    let failed = |error: std::io::Error| {
        LockFailure::Failed(format!("cannot lock {}: {error}", path.display()))
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(failed)?;
    let waited = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if waited.elapsed() < PATIENCE => {
                tokio::time::sleep(INTERVAL).await;
            }
            Err(TryLockError::WouldBlock) => return Err(LockFailure::Held(holder(db))),
            Err(TryLockError::Error(error)) => return Err(failed(error)),
        }
    }
    if let Err(error) = record_owner(db, endpoint) {
        eprintln!("[crowbotd] cannot record the owner of the database: {error}");
    }
    Ok(DatabaseLock { _file: file })
}

/// Writes which daemon holds the database, through a temporary file, so a
/// reader never sees half of it.
fn record_owner(db: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    let owner = Owner {
        pid: std::process::id(),
        endpoint: endpoint.address().to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    };
    let text = serde_json::to_vec(&owner).map_err(std::io::Error::other)?;
    let path = beside(db, "owner");
    let partial = beside(db, "owner.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, &path)
}

/// The message of a daemon that finds the database held.
fn holder(db: &Path) -> String {
    let owner = std::fs::read(beside(db, "owner"))
        .ok()
        .and_then(|text| serde_json::from_slice::<Owner>(&text).ok());
    match owner {
        Some(owner) => format!(
            "another crowbotd (process {}, endpoint {}) uses the database {}",
            owner.pid,
            owner.endpoint,
            db.display()
        ),
        None => format!("another crowbotd uses the database {}", db.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_lock_on_one_database_is_held_and_names_the_first() {
        let dir = std::env::temp_dir().join(format!("crowbotd-instance-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("crowbot.db");
        let endpoint = Endpoint::new("first-endpoint");
        let first = lock_database(&db, &endpoint).await;
        assert!(first.is_ok());
        let second = lock_database(&db, &Endpoint::new("second-endpoint")).await;
        let Err(LockFailure::Held(message)) = second else {
            panic!("the second lock was not refused as held");
        };
        assert!(
            message.contains(&format!("process {}", std::process::id())),
            "{message}"
        );
        assert!(message.contains("first-endpoint"), "{message}");
        // The lock ends with its holder.
        drop(first);
        assert!(lock_database(&db, &endpoint).await.is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
