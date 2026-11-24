//! Checkpoints (EP-008): before and after each turn the daemon records the
//! bot's folder, so that what a turn changed can be listed and put back,
//! whether an edit or a command changed it.
//!
//! A record is a tree of trees (`tree.rs`) stored in the database, and each
//! content it keeps is a file of `<data>/checkpoints/<database>/objects`, named by its
//! SHA-256 (`objects.rs`). A record that fails does not hold the turn back:
//! the conversation says why, and the turn runs. A record covers the bot's
//! folder only; what a turn changes elsewhere is not in it.
//!
//! The records and the restores of one folder take turns, under the lock of
//! that folder, and a sweep of the contents that no record names waits
//! until none of them writes. A record runs in the task of its conversation,
//! as the conversation's writes to the database do: the hashes of the last
//! record keep it to a walk of the folder when few files changed, and the
//! files it does read are hashed on several threads.

mod beneath;
mod ignored;
mod limits;
mod objects;
mod restore;
mod snapshot;
mod tree;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use crowbot_protocol::{
    CheckpointMoment, Event, FileChange, FileChangeKind, LeftCause, LeftFile, RESTORE_PATHS,
    RestoreRefusal, TurnChanges,
};

use crate::store::{CheckpointRow, NewCheckpoint, SharedStore, StoreError};
use beneath::Root;
use ignored::Ignored;
use objects::Objects;
pub use snapshot::Skip;
use snapshot::{FolderCache, Taken};
use tree::{Node, Trees};

/// Where a record is taken, and what it is tied to.
pub struct Moment<'a> {
    pub conversation_id: i64,
    pub bot_id: i64,
    pub workspace: &'a str,
    pub turn_event_id: Option<i64>,
    pub event_id: Option<i64>,
    pub moment: CheckpointMoment,
}

/// A restore a person asked for.
pub struct RestoreRequest<'a> {
    pub conversation_id: i64,
    pub bot_id: i64,
    pub workspace: &'a str,
    pub turn_event_id: i64,
    pub paths: Option<&'a [String]>,
    pub confirm: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub cause: RestoreRefusal,
    pub message: String,
    pub conflicts: Vec<String>,
}

impl Refused {
    fn new(cause: RestoreRefusal, message: impl Into<String>) -> Self {
        Self {
            cause,
            message: message.into(),
            conflicts: Vec::new(),
        }
    }
}

impl From<StoreError> for Refused {
    fn from(error: StoreError) -> Self {
        Self::new(RestoreRefusal::Other, error.to_string())
    }
}

#[derive(Default)]
struct FolderState {
    /// The hashes of the folder's last record, and the sweep it came after.
    cache: Option<(u64, FolderCache)>,
}

pub struct Checkpoints {
    objects: Objects,
    /// What a record leaves out: the daemon's own data and database.
    excluded: Vec<PathBuf>,
    folders: Mutex<HashMap<PathBuf, Arc<Mutex<FolderState>>>>,
    /// Held to read by a record or a restore, to write by a sweep.
    sweeping: RwLock<()>,
    /// How many sweeps ran: a cache made before the last one may name a
    /// content it removed.
    sweeps: AtomicU64,
}

/// The folder as the daemon reads it: its links resolved, so that two names
/// of one folder share one lock and one history.
pub fn folder_key(workspace: &str) -> Option<PathBuf> {
    std::fs::canonicalize(workspace).ok()
}

impl Checkpoints {
    pub fn new(root: PathBuf, excluded: Vec<PathBuf>) -> Self {
        let excluded = excluded
            .into_iter()
            .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
            .collect();
        Self {
            objects: Objects::new(root.join("objects")),
            excluded,
            folders: Mutex::new(HashMap::new()),
            sweeping: RwLock::new(()),
            sweeps: AtomicU64::new(0),
        }
    }

    fn folder(&self, key: &Path) -> Arc<Mutex<FolderState>> {
        self.folders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    fn take(&self, key: &Path, state: &mut FolderState) -> Result<Taken, Skip> {
        let sweeps = self.sweeps.load(Ordering::SeqCst);
        let cache = state
            .cache
            .as_ref()
            .filter(|(made, _)| *made == sweeps)
            .map(|(_, cache)| cache);
        snapshot::take(key, &self.excluded, &self.objects, cache)
    }

    fn store_taken(
        &self,
        store: &SharedStore,
        key: &Path,
        at: &Moment<'_>,
        taken: Taken,
        state: &mut FolderState,
    ) -> Result<i64, Skip> {
        let folder = key.to_string_lossy();
        let new = NewCheckpoint {
            conversation_id: at.conversation_id,
            bot_id: at.bot_id,
            turn_event_id: at.turn_event_id,
            event_id: at.event_id,
            moment: at.moment,
            folder: &folder,
            tree: &taken.tree,
            files: taken.files,
            bytes: taken.bytes,
            stored_bytes: taken.stored_bytes,
            taken_at_ms: taken.taken_at_ms,
            duration_ms: u64::try_from(taken.duration.as_millis()).unwrap_or(u64::MAX),
        };
        let id = store
            .with(|store| store.record_checkpoint(&new, &taken.trees))
            .map_err(Skip::storage)?;
        state.cache = Some((self.sweeps.load(Ordering::SeqCst), taken.cache));
        Ok(id)
    }

    /// Records the folder of a bot and stores the record, tied to its event.
    pub fn record(&self, store: &SharedStore, at: &Moment<'_>) -> Result<i64, Skip> {
        let key = folder_key(at.workspace).ok_or_else(|| Skip {
            cause: crowbot_protocol::CheckpointCause::FolderMissing,
            path: None,
            message: format!("The folder {} does not exist.", at.workspace),
        })?;
        let folder = self.folder(&key);
        let mut state = folder.lock().unwrap_or_else(PoisonError::into_inner);
        let _writing = self.sweeping.read().unwrap_or_else(PoisonError::into_inner);
        let taken = self.take(&key, &mut state)?;
        self.store_taken(store, &key, at, taken, &mut state)
    }

    /// What the turn that the `UserText` event `event_id` started changed in
    /// the bot's folder.
    pub fn changes(
        &self,
        store: &SharedStore,
        conversation_id: i64,
        event_id: i64,
    ) -> Result<TurnChanges, String> {
        store
            .with(|store| {
                if !store.is_turn_start(conversation_id, event_id)? {
                    return Ok(Err(format!(
                        "event {event_id} does not start a turn of conversation {conversation_id}"
                    )));
                }
                let read = |moment| store.turn_checkpoint(conversation_id, event_id, moment);
                let Some(before) = read(CheckpointMoment::BeforeTurn)? else {
                    return Ok(Err(
                        "The daemon took no record of the folder before this turn.".to_owned(),
                    ));
                };
                let Some(after) = read(CheckpointMoment::AfterTurn)? else {
                    return Ok(Err(
                        "The daemon took no record of the folder after this turn: it has not \
                         ended, or the record failed."
                            .to_owned(),
                    ));
                };
                let mut trees = Trees::new(store);
                let changes = tree::diff(&mut trees, &before.tree, &after.tree)?
                    .into_iter()
                    .map(|difference| FileChange {
                        kind: match (&difference.before, &difference.after) {
                            (None, _) => FileChangeKind::Added,
                            (_, None) => FileChangeKind::Removed,
                            _ => FileChangeKind::Changed,
                        },
                        path: difference.path,
                        before: difference.before.as_ref().and_then(Node::state),
                        after: difference.after.as_ref().and_then(Node::state),
                    })
                    .collect();
                Ok::<_, StoreError>(Ok(TurnChanges {
                    conversation_id,
                    event_id,
                    before: before.summary,
                    after: after.summary,
                    changes,
                }))
            })
            .map_err(|error| error.to_string())?
    }

    /// Puts files of the bot's folder back as the record taken before the
    /// turn holds them. `busy` tells whether a turn runs on that folder,
    /// asked under the folder's lock, so that no turn starts its record
    /// while the restore writes; `record` stores the `CheckpointRestored`
    /// event and gives its id. The id of that event when it was applied.
    pub fn restore(
        &self,
        store: &SharedStore,
        request: &RestoreRequest<'_>,
        busy: impl FnOnce() -> bool,
        record: impl FnOnce(Event) -> Result<i64, String>,
    ) -> Result<i64, Refused> {
        let paths = match request.paths {
            Some(paths) => Some(checked_paths(paths)?),
            None => None,
        };
        let target = store
            .with(|store| {
                store.turn_checkpoint(
                    request.conversation_id,
                    request.turn_event_id,
                    CheckpointMoment::BeforeTurn,
                )
            })?
            .ok_or_else(|| {
                Refused::new(
                    RestoreRefusal::NoCheckpoint,
                    "The daemon took no record of the folder before this turn.",
                )
            })?;
        let key = folder_key(request.workspace)
            .filter(|key| key.to_string_lossy() == target.folder)
            .ok_or_else(|| {
                Refused::new(
                    RestoreRefusal::FolderChanged,
                    format!(
                        "The turn ran in {}, and the bot now works in {}.",
                        target.folder, request.workspace
                    ),
                )
            })?;
        let folder = self.folder(&key);
        let mut state = folder.lock().unwrap_or_else(PoisonError::into_inner);
        if busy() {
            return Err(Refused::new(
                RestoreRefusal::TurnRunning,
                "A turn runs in this folder. Stop it, or wait for its end, then restore.",
            ));
        }
        let _writing = self.sweeping.read().unwrap_or_else(PoisonError::into_inner);
        let now = self
            .take(&key, &mut state)
            .map_err(|skip| Refused::new(RestoreRefusal::Other, skip.message))?;
        let plan = plan(
            store,
            &self.objects,
            &target,
            &now,
            &key,
            paths.as_deref(),
            request.confirm,
        )?;
        let at = Moment {
            conversation_id: request.conversation_id,
            bot_id: request.bot_id,
            workspace: request.workspace,
            turn_event_id: Some(request.turn_event_id),
            event_id: None,
            moment: CheckpointMoment::BeforeRestore,
        };
        // The folder as it stood comes first: a restore can be undone too.
        let before = self
            .store_taken(store, &key, &at, now, &mut state)
            .map_err(|skip| Refused::new(RestoreRefusal::Other, skip.message))?;
        let mut done = restore::apply(&key, &plan.differences, &plan.folders, &self.objects);
        done.left.extend(plan.held);
        let event_id = record(Event::CheckpointRestored {
            turn_event_id: request.turn_event_id,
            restored: done.restored,
            removed: done.removed,
            left: done.left,
        })
        // The files are written by now: the answer must not read as a restore
        // that left the folder alone.
        .map_err(|message| {
            Refused::new(
                RestoreRefusal::Other,
                format!(
                    "The files were put back, but the daemon could not record the restore in \
                     the conversation: {message}"
                ),
            )
        })?;
        let tied = store.with(|store| store.tie_checkpoint(before, event_id));
        if let Err(error) = tied {
            eprintln!("[crowbotd] cannot tie the record before a restore to its event: {error}");
        }
        // The folder as the restore left it is the last record, so a file
        // it wrote is not taken for a change by another hand.
        let after = Moment {
            event_id: Some(event_id),
            moment: CheckpointMoment::AfterRestore,
            ..at
        };
        if let Err(skip) = self
            .take(&key, &mut state)
            .and_then(|taken| self.store_taken(store, &key, &after, taken, &mut state))
        {
            eprintln!(
                "[crowbotd] cannot record the folder after a restore: {}",
                skip.message
            );
        }
        Ok(event_id)
    }

    /// Removes the trees and the contents that no record names any longer.
    /// The count of contents removed.
    pub fn sweep(&self, store: &SharedStore) -> Result<usize, StoreError> {
        let _sweeping = self
            .sweeping
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        self.sweeps.fetch_add(1, Ordering::SeqCst);
        let kept = store.with(|store| {
            let mut trees = Trees::new(store);
            let mut reached = HashSet::new();
            let mut contents = HashSet::new();
            let mut next = store.checkpoint_roots()?;
            while let Some(hash) = next.pop() {
                if !reached.insert(hash.clone()) {
                    continue;
                }
                for entry in trees.get(&hash)?.iter() {
                    match &entry.node {
                        Node::Folder { tree } => next.push(tree.clone()),
                        Node::File {
                            sha256, kept: true, ..
                        } => {
                            contents.insert(sha256.clone());
                        }
                        _ => {}
                    }
                }
            }
            let unreached: Vec<String> = store
                .checkpoint_tree_hashes()?
                .into_iter()
                .filter(|hash| !reached.contains(hash))
                .collect();
            store.delete_checkpoint_trees(&unreached)?;
            Ok::<_, StoreError>(contents)
        })?;
        Ok(self.objects.sweep(&kept))
    }
}

/// The paths a message names, as records write them. A path with an empty,
/// `.`, or `..` part, or that is absolute, names nothing inside the folder.
fn checked_paths(paths: &[String]) -> Result<Vec<String>, Refused> {
    if paths.len() > RESTORE_PATHS {
        return Err(Refused::new(
            RestoreRefusal::InvalidPath,
            format!("A restore names {RESTORE_PATHS} paths at most."),
        ));
    }
    paths
        .iter()
        .map(|path| {
            let path = path.replace('\\', "/");
            let path = path.trim_end_matches('/');
            let valid = !path.is_empty()
                && !path.starts_with('/')
                && !path.contains(':')
                && path
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..");
            if valid {
                Ok(path.to_owned())
            } else {
                Err(Refused::new(
                    RestoreRefusal::InvalidPath,
                    format!("{path} is not a path inside the bot's folder."),
                ))
            }
        })
        .collect()
}

/// What a restore writes, and what it leaves.
struct Plan {
    /// Each path to put back, `before` holding the record's state.
    differences: Vec<tree::Difference>,
    /// The folders of the record, which stay when they are left empty.
    folders: BTreeSet<String>,
    /// The files the folder holds and the record lacks, which a removal
    /// leaves because the record left them out (`ignored.rs`).
    held: Vec<LeftFile>,
}

/// What a restore writes: each difference between the folder as it stands
/// and the record, among the paths named, refused when one of them changed
/// since the folder's last record and the person did not confirm it. A file
/// whose content the restore would lose for good is such a change too: one
/// git ignores now, which no record holds, or one too large to keep.
fn plan(
    store: &SharedStore,
    objects: &Objects,
    target: &CheckpointRow,
    now: &Taken,
    key: &Path,
    paths: Option<&[String]>,
    confirm: bool,
) -> Result<Plan, Refused> {
    let folder = key.to_string_lossy();
    store.with(|store| {
        let mut trees = Trees::with_pending(store, &now.trees);
        let covers = |path: &str| {
            paths.is_none_or(|paths| {
                paths.iter().any(|named| {
                    path == named
                        || path
                            .strip_prefix(named.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
        };
        let recorded = tree::list(&mut trees, &target.tree)?;
        let ignored = Ignored::of(key, &recorded, objects);
        let mut held = Vec::new();
        let differences: Vec<tree::Difference> = tree::diff(&mut trees, &now.tree, &target.tree)?
            .into_iter()
            .filter(|difference| covers(&difference.path))
            .map(|difference| tree::Difference {
                // `before` is the state to write back: the record's.
                path: difference.path,
                before: difference.after,
                after: difference.before,
            })
            .filter(|difference| {
                let left_out = difference.before.is_none() && ignored.covers(key, &difference.path);
                if left_out {
                    held.push(LeftFile {
                        path: difference.path.clone(),
                        cause: LeftCause::Ignored,
                        message: None,
                    });
                }
                !left_out
            })
            .collect();
        if !confirm {
            let moved: HashSet<String> = match store.last_checkpoint(&folder)? {
                Some(last) => tree::diff(&mut trees, &last.tree, &now.tree)?
                    .into_iter()
                    .map(|difference| difference.path)
                    .collect(),
                None => HashSet::new(),
            };
            // What the restore would write over or remove and no record could
            // give back: a file git ignores now, or one whose content the
            // record of the folder as it stands did not keep.
            let root = Root::open(key).ok();
            let unrecorded = |difference: &tree::Difference| match &difference.after {
                None => {
                    difference.before.is_some()
                        && root
                            .as_ref()
                            .is_some_and(|root| root.holds(&difference.path))
                }
                Some(Node::File { kept, .. }) => !kept,
                Some(_) => false,
            };
            let conflicts: Vec<String> = differences
                .iter()
                .filter(|difference| moved.contains(&difference.path) || unrecorded(difference))
                .map(|difference| difference.path.clone())
                .collect();
            if !conflicts.is_empty() {
                return Err(Refused {
                    cause: RestoreRefusal::Conflicts,
                    message: format!(
                        "{} changed since the daemon last recorded the folder, or hold content it \
                         did not keep. Confirm to write over them.",
                        conflicts.join(", ")
                    ),
                    conflicts,
                });
            }
        }
        Ok(Plan {
            differences,
            folders: recorded.folders,
            held,
        })
    })
}

#[cfg(test)]
mod tests;
