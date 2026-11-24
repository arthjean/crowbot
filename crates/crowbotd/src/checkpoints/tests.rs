use std::fs;
use std::path::Path;

use crowbot_protocol::scratch::Scratch;
use crowbot_protocol::{
    Avatar, CheckpointCause, CheckpointMoment, Event, FileChangeKind, FileKind, LeftCause,
    MascotShape, PermissionLevel, RestoreRefusal,
};

use super::limits::CHECKPOINT_FILE_BYTES;
use super::*;
use crate::digest::sha256_hex;
use crate::store::NewBot;

struct Bench {
    scratch: Scratch,
    store: SharedStore,
    checkpoints: Checkpoints,
    bot_id: i64,
    conversation_id: i64,
    folder: String,
    next_event: i64,
}

impl Bench {
    fn new() -> Self {
        let scratch = Scratch::new("checkpoints");
        let folder = scratch.path().join("bot");
        fs::create_dir(&folder).unwrap();
        let store = SharedStore::in_memory();
        let row = store
            .with(|store| {
                store.create_bot(&NewBot {
                    name: "Atlas".to_owned(),
                    title: String::new(),
                    instructions: String::new(),
                    avatar: Avatar {
                        color: "#3b82f6".to_owned(),
                        shape: MascotShape::Smooth,
                    },
                    provider: "claude".to_owned(),
                    model: None,
                    effort: None,
                    permission_level: PermissionLevel::AskFirst,
                    tool_call_limit: None,
                    workspace: folder.to_string_lossy().into_owned(),
                })
            })
            .unwrap();
        let checkpoints = Checkpoints::new(scratch.path().join("data/checkpoints"), Vec::new());
        Self {
            folder: folder.to_string_lossy().into_owned(),
            scratch,
            store,
            checkpoints,
            bot_id: row.bot.id,
            conversation_id: row.current_conversation_id,
            next_event: 0,
        }
    }

    fn path(&self, path: &str) -> std::path::PathBuf {
        Path::new(&self.folder).join(path)
    }

    fn write(&self, path: &str, content: &[u8]) {
        let place = self.path(path);
        fs::create_dir_all(place.parent().unwrap()).unwrap();
        fs::write(place, content).unwrap();
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        fs::read(self.path(path)).ok()
    }

    /// Starts a turn: its `UserText` and the record before it.
    fn before(&mut self) -> i64 {
        let event = self
            .store
            .with(|store| store.append(self.conversation_id, &Event::user_text("go")))
            .unwrap();
        self.next_event = event;
        self.record(Some(event), CheckpointMoment::BeforeTurn)
            .unwrap();
        event
    }

    fn after(&self, turn: i64) {
        self.record(Some(turn), CheckpointMoment::AfterTurn)
            .unwrap();
    }

    fn record(&self, turn: Option<i64>, moment: CheckpointMoment) -> Result<i64, Skip> {
        self.checkpoints.record(
            &self.store,
            &Moment {
                conversation_id: self.conversation_id,
                bot_id: self.bot_id,
                workspace: &self.folder,
                turn_event_id: turn,
                event_id: turn,
                moment,
            },
        )
    }

    fn row(&self, id: i64) -> CheckpointRow {
        self.store
            .with(|store| {
                store.turn_checkpoint(self.conversation_id, id, CheckpointMoment::BeforeTurn)
            })
            .unwrap()
            .unwrap()
    }

    fn objects(&self) -> usize {
        let root = self.scratch.path().join("data/checkpoints/objects");
        fs::read_dir(root).map_or(0, |folders| {
            folders
                .flatten()
                .map(|folder| fs::read_dir(folder.path()).unwrap().count())
                .sum()
        })
    }

    fn restore(
        &self,
        turn: i64,
        paths: Option<&[String]>,
        confirm: bool,
    ) -> Result<Event, Refused> {
        let mut recorded = None;
        let store = self.store.clone();
        let conversation_id = self.conversation_id;
        self.checkpoints.restore(
            &self.store,
            &RestoreRequest {
                conversation_id,
                bot_id: self.bot_id,
                workspace: &self.folder,
                turn_event_id: turn,
                paths,
                confirm,
            },
            || false,
            |event| {
                recorded = Some(event.clone());
                store
                    .with(|store| store.append(conversation_id, &event))
                    .map_err(|error| error.to_string())
            },
        )?;
        Ok(recorded.unwrap())
    }

    fn listing(&self, turn: i64) -> tree::Listing {
        let row = self.row(turn);
        self.store
            .with(|store| tree::list(&mut Trees::new(store), &row.tree))
            .unwrap()
    }
}

#[test]
fn a_record_holds_each_file_with_its_size_and_hash() {
    let mut bench = Bench::new();
    bench.write("notes.txt", b"one");
    bench.write("src/main.rs", b"fn main() {}");
    let turn = bench.before();
    let listing = bench.listing(turn);
    let state = listing.files["src/main.rs"].state().unwrap();
    assert_eq!(state.kind, FileKind::File);
    assert_eq!(state.size, 12);
    assert_eq!(state.sha256, sha256_hex(b"fn main() {}"));
    assert!(state.kept);
    assert_eq!(listing.files["notes.txt"].state().unwrap().size, 3);
    assert!(listing.folders.contains("src"));
    let row = bench.row(turn);
    assert_eq!(row.summary.files, 2);
    assert_eq!(row.summary.bytes, 15);
    assert_eq!(row.summary.stored_bytes, 15);
}

#[test]
fn what_git_ignores_is_not_recorded() {
    let mut bench = Bench::new();
    bench.write(".gitignore", b"target/\n*.log\n");
    bench.write("target/debug/app", b"binary");
    bench.write("run.log", b"log");
    bench.write(".git/HEAD", b"ref: refs/heads/main");
    bench.write("sub/.gitignore", b"secret.txt\n");
    bench.write("sub/secret.txt", b"hidden");
    bench.write("sub/kept.txt", b"kept");
    bench.write(".hidden", b"dot files count");
    let turn = bench.before();
    let listing = bench.listing(turn);
    let paths: Vec<&str> = listing.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [".gitignore", ".hidden", "sub/.gitignore", "sub/kept.txt"]
    );
}

#[test]
fn a_content_is_stored_once_and_an_unchanged_folder_stores_nothing() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"same");
    bench.write("b.txt", b"same");
    let first = bench.before();
    assert_eq!(bench.objects(), 1);
    assert_eq!(bench.row(first).summary.stored_bytes, 4);
    bench.after(first);
    let second = bench.before();
    assert_eq!(bench.objects(), 1);
    assert_eq!(bench.row(second).summary.stored_bytes, 0);
    assert_eq!(bench.row(second).tree, bench.row(first).tree);
}

#[test]
fn a_large_file_has_its_hash_and_no_content() {
    let mut bench = Bench::new();
    let large = vec![b'x'; usize::try_from(CHECKPOINT_FILE_BYTES).unwrap() + 1];
    bench.write("large.bin", &large);
    let turn = bench.before();
    let state = bench.listing(turn).files["large.bin"].state().unwrap();
    assert!(!state.kept);
    assert_eq!(state.sha256, sha256_hex(&large));
    assert_eq!(state.size, CHECKPOINT_FILE_BYTES + 1);
    assert_eq!(bench.objects(), 0);
}

#[cfg(unix)]
#[test]
fn a_link_is_recorded_with_its_target_and_not_followed() {
    let mut bench = Bench::new();
    let outside = bench.scratch.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, bench.path("away")).unwrap();
    let turn = bench.before();
    let listing = bench.listing(turn);
    let state = listing.files["away"].state().unwrap();
    assert_eq!(state.kind, FileKind::Link);
    assert_eq!(state.target.as_deref(), outside.to_str());
    assert!(!listing.files.contains_key("away/secret.txt"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_makes_the_record_fail_with_its_path() {
    use std::os::unix::fs::PermissionsExt;

    if crate::private::is_root() {
        return;
    }
    let bench = Bench::new();
    bench.write("locked.txt", b"locked");
    fs::set_permissions(bench.path("locked.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    let skip = bench
        .record(None, CheckpointMoment::BeforeTurn)
        .unwrap_err();
    assert_eq!(skip.cause, CheckpointCause::Unreadable);
    assert_eq!(skip.path.as_deref(), Some("locked.txt"));
}

#[test]
fn a_store_that_cannot_be_written_makes_the_record_fail() {
    let bench = Bench::new();
    bench.write("a.txt", b"a");
    // A file where the contents' folder goes.
    fs::create_dir_all(bench.scratch.path().join("data")).unwrap();
    fs::write(bench.scratch.path().join("data/checkpoints"), b"in the way").unwrap();
    let skip = bench
        .record(None, CheckpointMoment::BeforeTurn)
        .unwrap_err();
    assert_eq!(skip.cause, CheckpointCause::Storage);
}

#[test]
fn a_turn_lists_what_it_added_changed_and_removed() {
    let mut bench = Bench::new();
    bench.write("kept.txt", b"kept");
    bench.write("changed.txt", b"old");
    bench.write("gone/removed.txt", b"bye");
    let turn = bench.before();
    bench.write("changed.txt", b"new");
    fs::remove_dir_all(bench.path("gone")).unwrap();
    bench.write("added.txt", b"hello");
    bench.after(turn);
    let changes = bench
        .checkpoints
        .changes(&bench.store, bench.conversation_id, turn)
        .unwrap();
    let listed: Vec<(&str, FileChangeKind)> = changes
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("added.txt", FileChangeKind::Added),
            ("changed.txt", FileChangeKind::Changed),
            ("gone/removed.txt", FileChangeKind::Removed),
        ]
    );
    let changed = &changes.changes[1];
    assert_eq!(changed.before.as_ref().unwrap().sha256, sha256_hex(b"old"));
    assert_eq!(changed.after.as_ref().unwrap().sha256, sha256_hex(b"new"));
}

#[test]
fn a_restore_puts_the_folder_back_and_removes_what_the_turn_added() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"one");
    bench.write("deep/b.txt", b"two");
    let turn = bench.before();
    bench.write("a.txt", b"changed");
    fs::remove_file(bench.path("deep/b.txt")).unwrap();
    bench.write("new/c.txt", b"added");
    bench.after(turn);
    let Event::CheckpointRestored {
        restored,
        removed,
        left,
        ..
    } = bench.restore(turn, None, false).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(restored, ["a.txt", "deep/b.txt"]);
    assert_eq!(removed, ["new/c.txt"]);
    assert!(left.is_empty());
    assert_eq!(bench.read("a.txt").as_deref(), Some(&b"one"[..]));
    assert_eq!(bench.read("deep/b.txt").as_deref(), Some(&b"two"[..]));
    assert!(
        !bench.path("new").exists(),
        "the folder the turn added is gone"
    );
    // The folder as it stood before the restore is recorded, and then as
    // the restore left it.
    let folder = bench.row(turn).folder;
    let moments = bench.store.with(|store| {
        let last = store.last_checkpoint(&folder).unwrap().unwrap();
        (
            last.summary.moment,
            store
                .turn_checkpoint(bench.conversation_id, turn, CheckpointMoment::BeforeRestore)
                .unwrap()
                .is_some(),
        )
    });
    assert_eq!(moments, (CheckpointMoment::AfterRestore, true));
}

#[test]
fn a_restore_of_named_paths_leaves_the_others() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"one");
    bench.write("dir/b.txt", b"two");
    let turn = bench.before();
    bench.write("a.txt", b"changed");
    bench.write("dir/b.txt", b"changed");
    bench.after(turn);
    bench
        .restore(turn, Some(&["dir".to_owned()]), false)
        .unwrap();
    assert_eq!(bench.read("a.txt").as_deref(), Some(&b"changed"[..]));
    assert_eq!(bench.read("dir/b.txt").as_deref(), Some(&b"two"[..]));
}

#[test]
fn a_file_changed_since_the_last_record_needs_a_confirmation() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"one");
    let turn = bench.before();
    bench.write("a.txt", b"by the turn");
    bench.after(turn);
    // Another hand, after the turn.
    bench.write("a.txt", b"by a person, longer");
    let refused = bench.restore(turn, None, false).unwrap_err();
    assert_eq!(refused.cause, RestoreRefusal::Conflicts);
    assert_eq!(refused.conflicts, ["a.txt"]);
    assert_eq!(
        bench.read("a.txt").as_deref(),
        Some(&b"by a person, longer"[..])
    );
    // A refusal stores no record, so asking again is refused again.
    let again = bench.restore(turn, None, false).unwrap_err();
    assert_eq!(again.conflicts, ["a.txt"]);
    bench.restore(turn, None, true).unwrap();
    assert_eq!(bench.read("a.txt").as_deref(), Some(&b"one"[..]));
    // A second restore right after finds nothing changed by another hand.
    bench.restore(turn, None, false).unwrap();
}

#[test]
fn a_file_with_no_content_kept_is_left_and_named() {
    let mut bench = Bench::new();
    let large = vec![b'x'; usize::try_from(CHECKPOINT_FILE_BYTES).unwrap() + 1];
    bench.write("large.bin", &large);
    bench.write("small.txt", b"small");
    let turn = bench.before();
    bench.write("large.bin", b"shrunk");
    bench.write("small.txt", b"changed");
    bench.after(turn);
    let Event::CheckpointRestored { restored, left, .. } =
        bench.restore(turn, None, false).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(restored, ["small.txt"]);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].path, "large.bin");
    assert_eq!(left[0].cause, LeftCause::NotKept);
    assert_eq!(bench.read("large.bin").as_deref(), Some(&b"shrunk"[..]));
}

#[cfg(unix)]
#[test]
fn a_restore_never_writes_through_a_link() {
    let mut bench = Bench::new();
    bench.write("dir/a.txt", b"one");
    let turn = bench.before();
    fs::remove_dir_all(bench.path("dir")).unwrap();
    let outside = bench.scratch.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, bench.path("dir")).unwrap();
    bench.after(turn);
    let Event::CheckpointRestored { left, .. } = bench
        .restore(turn, Some(&["dir/a.txt".to_owned()]), false)
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].cause, LeftCause::Failed);
    assert!(!outside.join("a.txt").exists());
}

#[test]
fn a_restore_refuses_a_path_outside_the_folder() {
    let mut bench = Bench::new();
    let turn = bench.before();
    bench.after(turn);
    for path in ["../x", "/etc/passwd", "a/./b", ""] {
        let refused = bench
            .restore(turn, Some(&[path.to_owned()]), false)
            .unwrap_err();
        assert_eq!(refused.cause, RestoreRefusal::InvalidPath, "{path}");
    }
}

#[test]
fn a_sweep_removes_what_no_record_names() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"one");
    let turn = bench.before();
    assert_eq!(bench.objects(), 1);
    bench.conversation_id = bench
        .store
        .with(|store| store.delete_history(bench.bot_id))
        .unwrap()
        .unwrap();
    assert_eq!(bench.checkpoints.sweep(&bench.store).unwrap(), 1);
    assert_eq!(bench.objects(), 0);
    assert!(
        bench
            .store
            .with(|store| store.checkpoint_tree_hashes())
            .unwrap()
            .is_empty()
    );
    let _ = turn;
    // A record after the sweep stores the content again.
    bench.before();
    assert_eq!(bench.objects(), 1);
}

#[test]
fn a_restore_never_removes_a_file_the_record_left_out_as_ignored() {
    let mut bench = Bench::new();
    bench.write(".gitignore", b"secret.env\n*.log\n!keep.log\n");
    bench.write("secret.env", b"KEY=1");
    bench.write("deep/.gitignore", b"cache/\n");
    bench.write("deep/cache/data.bin", b"cached");
    let turn = bench.before();
    // The turn stops ignoring them, and adds files of its own.
    bench.write(".gitignore", b"");
    bench.write("deep/.gitignore", b"");
    bench.write("keep.log", b"added");
    bench.write("new.txt", b"added");
    bench.after(turn);
    let Event::CheckpointRestored {
        restored,
        removed,
        left,
        ..
    } = bench.restore(turn, None, false).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(restored, [".gitignore", "deep/.gitignore"]);
    // `keep.log` was not ignored before the turn: the turn added it.
    assert_eq!(removed, ["keep.log", "new.txt"]);
    let left: Vec<(&str, LeftCause)> = left
        .iter()
        .map(|file| (file.path.as_str(), file.cause))
        .collect();
    assert_eq!(
        left,
        [
            ("deep/cache/data.bin", LeftCause::Ignored),
            ("secret.env", LeftCause::Ignored)
        ]
    );
    assert_eq!(bench.read("secret.env").as_deref(), Some(&b"KEY=1"[..]));
    assert_eq!(
        bench.read("deep/cache/data.bin").as_deref(),
        Some(&b"cached"[..])
    );
}

#[test]
fn a_restore_over_a_file_git_ignores_now_needs_a_confirmation() {
    let mut bench = Bench::new();
    bench.write("notes.txt", b"before");
    let turn = bench.before();
    // The turn hides the file from git, then another hand writes it: no
    // record holds what it holds now.
    bench.write(".gitignore", b"notes.txt\n");
    bench.after(turn);
    bench.write("notes.txt", b"unrecorded work");
    let refused = bench.restore(turn, None, false).unwrap_err();
    assert_eq!(refused.cause, RestoreRefusal::Conflicts);
    assert_eq!(refused.conflicts, ["notes.txt"]);
    assert_eq!(
        bench.read("notes.txt").as_deref(),
        Some(&b"unrecorded work"[..])
    );
    bench.restore(turn, None, true).unwrap();
    assert_eq!(bench.read("notes.txt").as_deref(), Some(&b"before"[..]));
    assert_eq!(bench.read(".gitignore"), None);
}

#[test]
fn a_restore_over_a_file_whose_content_no_record_kept_needs_a_confirmation() {
    let mut bench = Bench::new();
    bench.write("a.txt", b"one");
    let first = bench.before();
    bench.after(first);
    // A person adds a file too large to keep, and the next turn finds it.
    let large = vec![b'x'; usize::try_from(CHECKPOINT_FILE_BYTES).unwrap() + 1];
    bench.write("data.csv", &large);
    let second = bench.before();
    bench.after(second);
    let refused = bench.restore(first, None, false).unwrap_err();
    assert_eq!(refused.cause, RestoreRefusal::Conflicts);
    assert_eq!(refused.conflicts, ["data.csv"]);
    assert_eq!(bench.read("data.csv").as_deref(), Some(&large[..]));
    let Event::CheckpointRestored { removed, .. } = bench.restore(first, None, true).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(removed, ["data.csv"]);
}
