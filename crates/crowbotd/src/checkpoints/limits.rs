//! The figures of checkpoints. The daemon's tests read this file too, so that
//! a test and the code share each figure.

use std::time::Duration;

/// The largest file whose content a record keeps: the per-file limit of a
/// skill (`SKILL_FILE_BYTES`, US-022). A larger file has its hash alone.
pub const CHECKPOINT_FILE_BYTES: u64 = 1 << 20;
/// The most entries (files, links, and folders) a record takes. A folder
/// that holds more is not recorded, and the turn says so.
pub const CHECKPOINT_ENTRIES: usize = 100_000;
/// The most bytes of new content one record stores. Past it, the files
/// whose content no record kept have their hash alone.
pub const CHECKPOINT_NEW_BYTES: u64 = 256 << 20;
/// A file changed this close to the start of the last record is hashed
/// again, even when its size and times did not change: a file system may
/// keep its times this coarsely, and a write within that span leaves them
/// as they were.
pub const RACY_MARGIN: Duration = Duration::from_secs(2);
/// The most threads that hash the files of one record.
pub const HASH_THREADS: usize = 8;
/// The bytes to read that make one more thread worth starting.
pub const HASH_THREAD_BYTES: u64 = 8 << 20;
