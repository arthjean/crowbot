//! Takes a record of a folder: walks it as git would list it (no `.git`,
//! nothing a `.gitignore` of the folder matches, links not followed),
//! hashes each file, keeps each new content once, and builds the trees.
//!
//! A file whose size and times did not move since the last record is not
//! read again: its hash comes from that record, as git's index does. A file
//! changed within `RACY_MARGIN` of that record's start is read again
//! whatever its times say.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crowbot_protocol::CheckpointCause;
use sha2::{Digest, Sha256};

use super::beneath::{Root, is_moved};
use super::limits::{
    CHECKPOINT_ENTRIES, CHECKPOINT_FILE_BYTES, CHECKPOINT_NEW_BYTES, HASH_THREAD_BYTES,
    HASH_THREADS, RACY_MARGIN,
};
use super::objects::Objects;
use super::tree::{Entry, Node, encode};
use crate::digest::hex;

/// Why a record could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub cause: CheckpointCause,
    /// The path that could not be read, relative to the folder.
    pub path: Option<String>,
    pub message: String,
}

impl Skip {
    pub fn storage(error: impl std::fmt::Display) -> Self {
        Self {
            cause: CheckpointCause::Storage,
            path: None,
            message: format!("The daemon could not store the record of the folder: {error}"),
        }
    }
}

/// What the file system says of a file, to tell whether it may have changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified_ns: i128,
    changed_ns: i128,
    inode: u64,
    executable: bool,
    mode: u32,
}

#[derive(Debug, Clone)]
struct Hashed {
    stamp: Stamp,
    size: u64,
    sha256: String,
    kept: bool,
}

/// The hashes of the last record of a folder, with when it started.
#[derive(Debug, Default)]
pub struct FolderCache {
    started_ns: i128,
    files: HashMap<String, Hashed>,
}

/// A record, ready to store.
pub struct Taken {
    pub tree: String,
    /// Every tree of the record, as `(hash, body)`.
    pub trees: Vec<(String, String)>,
    pub files: u64,
    pub bytes: u64,
    pub stored_bytes: u64,
    pub taken_at_ms: i64,
    pub duration: Duration,
    pub cache: FolderCache,
}

enum Kind {
    Folder,
    Link(String),
    File(Stamp),
}

fn nanos(time: std::io::Result<SystemTime>) -> i128 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos() as i128)
}

fn stamp(metadata: &std::fs::Metadata) -> Stamp {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Stamp {
            size: metadata.len(),
            modified_ns: nanos(metadata.modified()),
            changed_ns: i128::from(metadata.ctime()) * 1_000_000_000
                + i128::from(metadata.ctime_nsec()),
            inode: metadata.ino(),
            executable: metadata.mode() & 0o111 != 0,
            mode: metadata.mode() & 0o777,
        }
    }
    #[cfg(not(unix))]
    Stamp {
        size: metadata.len(),
        modified_ns: nanos(metadata.modified()),
        changed_ns: nanos(metadata.created()),
        inode: 0,
        executable: false,
        mode: if metadata.permissions().readonly() {
            0o444
        } else {
            0o666
        },
    }
}

/// The path relative to the folder, with `/` between its parts. `None` for
/// a name that is not UTF-8, which a record cannot hold.
pub fn relative(folder: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(folder)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    let parts = parts?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn error_path(error: &ignore::Error) -> Option<PathBuf> {
    match error {
        ignore::Error::WithPath { path, .. } => Some(path.clone()),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child.clone()),
        _ => None,
    }
}

fn unreadable(folder: &Path, path: Option<&Path>, error: impl std::fmt::Display) -> Skip {
    let path = path.and_then(|path| relative(folder, path));
    Skip {
        cause: CheckpointCause::Unreadable,
        message: match &path {
            Some(path) => format!("The daemon could not read {path}: {error}"),
            None => format!("The daemon could not read the folder: {error}"),
        },
        path,
    }
}

/// Records the folder. `excluded` holds paths the record leaves out (the
/// daemon's own data, when it lies inside the folder); `cache` is the last
/// record of the same folder.
pub fn take(
    folder: &Path,
    excluded: &[PathBuf],
    objects: &Objects,
    cache: Option<&FolderCache>,
) -> Result<Taken, Skip> {
    let clock = Instant::now();
    let started = SystemTime::now();
    let started_ns = nanos(Ok(started));
    if !folder.is_dir() {
        return Err(Skip {
            cause: CheckpointCause::FolderMissing,
            path: None,
            message: format!("The folder {} does not exist.", folder.display()),
        });
    }
    let entries = walk(folder, excluded)?;
    let root = Root::open(folder).map_err(|error| unreadable(folder, None, error))?;
    let hashed = hash(folder, &root, &entries, objects, cache)?;
    let mut taken = build(&entries, &hashed);
    taken.taken_at_ms = i64::try_from(started_ns / 1_000_000).unwrap_or(i64::MAX);
    taken.stored_bytes = hashed.stored;
    taken.duration = clock.elapsed();
    taken.cache = FolderCache {
        started_ns,
        files: hashed.files,
    };
    Ok(taken)
}

fn walk(folder: &Path, excluded: &[PathBuf]) -> Result<Vec<(String, Kind)>, Skip> {
    let mut builder = ignore::WalkBuilder::new(folder);
    // A `.gitignore` of the folder applies, whether or not the folder is a
    // repository; nothing above the folder or of the user's git settings
    // does.
    builder
        .standard_filters(false)
        .git_ignore(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    let excluded = excluded.to_vec();
    builder.filter_entry(move |entry| {
        entry.file_name() != ".git" && !excluded.iter().any(|path| entry.path() == path)
    });
    let mut entries = Vec::new();
    for result in builder.build() {
        let entry = result.map_err(|error| {
            let path = error_path(&error);
            unreadable(folder, path.as_deref(), &error)
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let Some(path) = relative(folder, entry.path()) else {
            // A name that is not UTF-8 is left out, with what it holds.
            continue;
        };
        let kind = if file_type.is_symlink() {
            let target = std::fs::read_link(entry.path())
                .map_err(|error| unreadable(folder, Some(entry.path()), error))?;
            match target.to_str() {
                Some(target) => Kind::Link(target.to_owned()),
                None => continue,
            }
        } else if file_type.is_dir() {
            Kind::Folder
        } else if file_type.is_file() {
            let metadata = entry
                .metadata()
                .map_err(|error| unreadable(folder, Some(entry.path()), error))?;
            Kind::File(stamp(&metadata))
        } else {
            // A socket, a pipe, or a device holds nothing to put back.
            continue;
        };
        entries.push((path, kind));
        if entries.len() > CHECKPOINT_ENTRIES {
            return Err(Skip {
                cause: CheckpointCause::TooManyFiles,
                path: None,
                message: format!(
                    "The folder holds more than {CHECKPOINT_ENTRIES} files and folders that git \
                     does not ignore. Add a .gitignore for what need not be put back."
                ),
            });
        }
    }
    Ok(entries)
}

struct HashedFiles {
    files: HashMap<String, Hashed>,
    stored: u64,
}

/// Hashes the files whose stamp moved, or whose content fits and was not
/// kept, on several threads, keeping each new
/// content that fits. A file that is gone by then is left out.
fn hash(
    folder: &Path,
    root: &Root,
    entries: &[(String, Kind)],
    objects: &Objects,
    cache: Option<&FolderCache>,
) -> Result<HashedFiles, Skip> {
    let margin = RACY_MARGIN.as_nanos() as i128;
    let mut files = HashMap::new();
    let mut jobs = Vec::new();
    for (path, kind) in entries {
        let Kind::File(stamp) = kind else {
            continue;
        };
        let known = cache.and_then(|cache| {
            let hashed = cache.files.get(path)?;
            let settled = stamp.modified_ns < cache.started_ns - margin
                && stamp.changed_ns < cache.started_ns - margin;
            // A content kept must still be there, and one that fits but was
            // not kept (the record had stored `CHECKPOINT_NEW_BYTES`) is read
            // again, so that a later record keeps it.
            let content = if hashed.kept {
                objects.contains(&hashed.sha256)
            } else {
                hashed.size > CHECKPOINT_FILE_BYTES
            };
            (hashed.stamp == *stamp && settled && content).then(|| hashed.clone())
        });
        match known {
            Some(hashed) => {
                files.insert(path.clone(), hashed);
            }
            None => jobs.push((path.as_str(), *stamp)),
        }
    }
    let next = AtomicUsize::new(0);
    let stored = Storing {
        bytes: AtomicU64::new(0),
        claimed: Mutex::new(HashMap::new()),
    };
    let results = Mutex::new(Vec::with_capacity(jobs.len()));
    let work = || {
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some((path, stamp)) = jobs.get(index) else {
                break;
            };
            let result = hash_file(root, path, *stamp, objects, &stored);
            let failed = result.is_err();
            results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((*path, result));
            if failed {
                // One failure ends the record: the others need not run.
                next.store(jobs.len(), Ordering::Relaxed);
                break;
            }
        }
    };
    // A thread per `HASH_THREAD_BYTES` to read, so that a few small files
    // are hashed where the record runs, with no thread to start.
    let bytes: u64 = jobs.iter().map(|(_, stamp)| stamp.size).sum();
    let threads = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(HASH_THREADS)
        .min(
            usize::try_from(bytes / HASH_THREAD_BYTES)
                .unwrap_or(usize::MAX)
                .max(1),
        )
        .min(jobs.len().max(1));
    if threads == 1 {
        work();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(work);
            }
        });
    }
    let results = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (path, result) in results {
        match result {
            Ok(Some(hashed)) => {
                files.insert(path.to_owned(), hashed);
            }
            Ok(None) => {}
            Err(Failure::Read(error)) => {
                return Err(unreadable(folder, Some(&folder.join(path)), error));
            }
            Err(Failure::Store(error)) => return Err(Skip::storage(error)),
        }
    }
    Ok(HashedFiles {
        files,
        stored: stored.bytes.into_inner(),
    })
}

/// What the threads of one record have stored: its bytes, and the hashes
/// whether each content read is kept, decided by the first thread that read it.
struct Storing {
    bytes: AtomicU64,
    claimed: Mutex<HashMap<String, bool>>,
}

enum Failure {
    Read(std::io::Error),
    Store(std::io::Error),
}

/// Reads a file once: a content that fits is hashed from memory and kept
/// from the same bytes, so the hash always names what was kept.
///
/// The file is opened from a handle on the bot's folder, with no link
/// followed (`beneath.rs`): a link, a pipe, or a device put in its place
/// since the walk, or a folder above it swapped for a link, is left out and
/// does not hold the record waiting.
fn hash_file(
    root: &Root,
    path: &str,
    stamp: Stamp,
    objects: &Objects,
    stored: &Storing,
) -> Result<Option<Hashed>, Failure> {
    let file = match root.open_file(path) {
        Ok(file) => file,
        Err(error) if is_moved(&error) => return Ok(None),
        Err(error) => return Err(Failure::Read(error)),
    };
    if !file.metadata().map_err(Failure::Read)?.is_file() {
        // No longer a file: left out, as a pipe or a device is.
        return Ok(None);
    }
    let mut reader = file.take(CHECKPOINT_FILE_BYTES + 1);
    let mut head = Vec::new();
    reader.read_to_end(&mut head).map_err(Failure::Read)?;
    let mut hasher = Sha256::new();
    hasher.update(&head);
    let mut size = head.len() as u64;
    if size > CHECKPOINT_FILE_BYTES {
        let mut file = reader.into_inner();
        let mut buffer = vec![0; 64 << 10];
        loop {
            let read = file.read(&mut buffer).map_err(Failure::Read)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            size += read as u64;
        }
        return Ok(Some(Hashed {
            stamp,
            size,
            sha256: hex(&hasher.finalize()),
            kept: false,
        }));
    }
    let sha256 = hex(&hasher.finalize());
    // Whether the content is kept is decided once per content, by the first
    // thread that reads it, so that two files of one content store it once
    // and agree on it.
    let (kept, write) = {
        let mut claimed = stored
            .claimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match claimed.get(&sha256) {
            Some(kept) => (*kept, false),
            None => {
                let held = objects.contains(&sha256);
                let room =
                    !held && stored.bytes.load(Ordering::Relaxed) + size <= CHECKPOINT_NEW_BYTES;
                if room {
                    stored.bytes.fetch_add(size, Ordering::Relaxed);
                }
                claimed.insert(sha256.clone(), held || room);
                (held || room, room)
            }
        }
    };
    if write && !objects.put(&sha256, &head).map_err(Failure::Store)? {
        // Another record wrote it meanwhile.
        stored.bytes.fetch_sub(size, Ordering::Relaxed);
    }
    Ok(Some(Hashed {
        stamp,
        size,
        sha256,
        kept,
    }))
}

/// Builds the tree of each folder, the deepest first, up to the root's.
fn build(entries: &[(String, Kind)], hashed: &HashedFiles) -> Taken {
    let parent_of = |path: &str| match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_owned(), name.to_owned()),
        None => (String::new(), path.to_owned()),
    };
    let mut children: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut folders = vec![String::new()];
    let (mut files, mut bytes) = (0, 0);
    for (path, kind) in entries {
        let node = match kind {
            Kind::Folder => {
                folders.push(path.clone());
                continue;
            }
            Kind::Link(target) => Node::Link {
                target: target.clone(),
            },
            Kind::File(_) => {
                let Some(hashed) = hashed.files.get(path) else {
                    continue;
                };
                bytes += hashed.size;
                Node::File {
                    size: hashed.size,
                    sha256: hashed.sha256.clone(),
                    executable: hashed.stamp.executable,
                    mode: Some(hashed.stamp.mode),
                    kept: hashed.kept,
                }
            }
        };
        files += 1;
        let (parent, name) = parent_of(path);
        children
            .entry(parent)
            .or_default()
            .push(Entry { name, node });
    }
    // The deepest folders first, so each tree is built before its parent's.
    folders.sort_by_key(|path| {
        std::cmp::Reverse(path.matches('/').count() + usize::from(!path.is_empty()))
    });
    let mut trees = Vec::with_capacity(folders.len());
    let mut root = String::new();
    for folder in folders {
        let mut entries = children.remove(&folder).unwrap_or_default();
        let (hash, body) = encode(&mut entries);
        trees.push((hash.clone(), body));
        if folder.is_empty() {
            root = hash;
        } else {
            let (parent, name) = parent_of(&folder);
            children.entry(parent).or_default().push(Entry {
                name,
                node: Node::Folder { tree: hash },
            });
        }
    }
    Taken {
        tree: root,
        trees,
        files,
        bytes,
        stored_bytes: 0,
        taken_at_ms: 0,
        duration: Duration::ZERO,
        cache: FolderCache::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::scratch::Scratch;

    #[test]
    fn a_content_left_out_for_room_is_kept_by_a_later_record() {
        let dir = Scratch::new("snapshot");
        let folder = dir.path().join("bot");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), b"one").unwrap();
        let objects = Objects::new(dir.path().join("objects"));
        let first = take(&folder, &[], &objects, None).unwrap();
        // The cache of a record that had no room left for this content.
        let mut cache = first.cache;
        let hashed = cache.files.get_mut("a.txt").unwrap();
        hashed.kept = false;
        cache.started_ns += 2 * RACY_MARGIN.as_nanos() as i128;
        std::fs::remove_dir_all(dir.path().join("objects")).unwrap();
        let second = take(&folder, &[], &objects, Some(&cache)).unwrap();
        assert!(second.cache.files["a.txt"].kept);
        assert!(objects.contains(&second.cache.files["a.txt"].sha256));
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_or_a_link_put_in_place_of_a_file_is_not_read() {
        let dir = Scratch::new("snapshot");
        let objects = Objects::new(dir.path().join("objects"));
        let stored = Storing {
            bytes: AtomicU64::new(0),
            claimed: Mutex::new(HashMap::new()),
        };
        let stamp = Stamp {
            size: 0,
            modified_ns: 0,
            changed_ns: 0,
            inode: 0,
            executable: false,
            mode: 0o600,
        };
        let folder = dir.path().join("bot");
        std::fs::create_dir(&folder).unwrap();
        let root = Root::open(&folder).unwrap();
        let pipe = folder.join("pipe");
        let name = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `name` is a valid C string that lives through the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            hash_file(&root, "pipe", stamp, &objects, &stored),
            Ok(None)
        ));
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), folder.join("link")).unwrap();
        assert!(matches!(
            hash_file(&root, "link", stamp, &objects, &stored),
            Ok(None)
        ));
        std::os::unix::fs::symlink(&outside, folder.join("sub")).unwrap();
        assert!(matches!(
            hash_file(&root, "sub/secret", stamp, &objects, &stored),
            Ok(None)
        ));
    }
}
