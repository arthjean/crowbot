//! EP-008 of the harness PRD: checkpoints, through the real daemon and the
//! fake agent. The daemon records the bot's folder before and after each
//! turn, lists what a turn changed whether an edit or a command changed it,
//! and puts files back as they were before a turn, never over a file that
//! another hand changed without a confirmation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crowbot_protocol::{
    BotDraft, BotState, CheckpointCause, CheckpointMoment, ClientMessage, Event, FileChangeKind,
    LeftCause, PermissionLevel, RestoreRefusal, ServerMessage, TurnChanges,
};

use crate::support::{Client, Daemon, eventually};

#[allow(dead_code)]
#[path = "../../src/checkpoints/limits.rs"]
mod limits;

use limits::CHECKPOINT_FILE_BYTES;

fn sha256(content: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A bot on Full access in a folder of its own, with its conversation open.
async fn bot(daemon: &Daemon, client: &mut Client) -> (i64, i64, PathBuf) {
    let folder = daemon.folder("work");
    let bot = client
        .create_bot(BotDraft {
            name: "Atlas".to_owned(),
            permission_level: Some(PermissionLevel::FullAccess),
            workspace: Some(folder.to_string_lossy().into_owned()),
            ..BotDraft::default()
        })
        .await;
    client.open(bot.current_conversation_id).await;
    (bot.bot.id, bot.current_conversation_id, folder)
}

/// Runs a turn, and returns the id of the `UserText` that started it.
async fn turn(client: &mut Client, conversation_id: i64, text: &str) -> i64 {
    client.ask(conversation_id, text).await;
    let snapshot = client.open(conversation_id).await;
    snapshot
        .events
        .iter()
        .rev()
        .find(|timed| matches!(&timed.event, Event::UserText(message) if message.text == text))
        .expect("the turn's message is stored")
        .id
}

async fn changes(client: &mut Client, conversation_id: i64, event_id: i64) -> TurnChanges {
    client
        .send(ClientMessage::GetTurnChanges {
            conversation_id,
            event_id,
        })
        .await;
    client
        .until(|message| match message {
            ServerMessage::TurnChanges(changes) => Some(changes),
            ServerMessage::Error { message } => panic!("no changes: {message}"),
            _ => None,
        })
        .await
}

enum Restore {
    Applied(Event),
    Refused(RestoreRefusal, Vec<String>),
}

async fn restore(
    client: &mut Client,
    conversation_id: i64,
    event_id: i64,
    paths: Option<Vec<String>>,
    confirm: bool,
) -> Restore {
    client
        .send(ClientMessage::RestoreCheckpoint {
            conversation_id,
            event_id,
            paths,
            confirm,
        })
        .await;
    let mut restored = None;
    loop {
        match client.next().await {
            ServerMessage::ConversationEvent {
                event: event @ Event::CheckpointRestored { .. },
                ..
            } => restored = Some(event),
            ServerMessage::RestoreApplied { .. } => {
                return Restore::Applied(restored.expect("the event comes before the answer"));
            }
            ServerMessage::RestoreRefused {
                cause, conflicts, ..
            } => return Restore::Refused(cause, conflicts),
            ServerMessage::Error { message } => panic!("the restore failed: {message}"),
            _ => {}
        }
    }
}

/// The contents stored, in the folder of the daemon's database.
fn objects(daemon: &Daemon) -> usize {
    let count = |root: PathBuf| {
        std::fs::read_dir(root).map_or(0, |folders| {
            folders
                .flatten()
                .map(|folder| std::fs::read_dir(folder.path()).map_or(0, Iterator::count))
                .sum()
        })
    };
    std::fs::read_dir(daemon.data_dir().join("checkpoints")).map_or(0, |databases| {
        databases
            .flatten()
            .map(|database| count(database.path().join("objects")))
            .sum()
    })
}

fn read(folder: &Path, name: &str) -> String {
    std::fs::read_to_string(folder.join(name)).unwrap_or_default()
}

/// The command that writes `text` to a file, for the shell of the system.
fn overwrite(file: &str, text: &str) -> String {
    format!("echo {text}> {file}")
}

#[tokio::test]
async fn a_turn_is_recorded_around_it_and_lists_what_an_edit_and_a_command_changed() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    std::fs::write(folder.join("b.txt"), "two").unwrap();
    std::fs::write(folder.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::create_dir_all(folder.join(".git")).unwrap();
    std::fs::write(folder.join(".git/HEAD"), "ref").unwrap();

    let edit = turn(&mut client, conversation, "edit a.txt by an edit").await;
    let edited = changes(&mut client, conversation, edit).await;
    assert_eq!(edited.changes.len(), 1, "{:?}", edited.changes);
    let change = &edited.changes[0];
    assert_eq!(
        (change.path.as_str(), change.kind),
        ("a.txt", FileChangeKind::Changed)
    );
    // The record before the turn holds the file as the agent found it: it
    // was taken before the prompt reached the agent.
    let before = change.before.as_ref().unwrap();
    assert_eq!(
        (before.size, before.sha256.as_str()),
        (3, sha256(b"one").as_str())
    );
    assert!(before.kept);
    let after = change.after.as_ref().unwrap();
    assert_eq!(after.sha256, sha256(b"by an edit"));
    assert_eq!(edited.before.moment, CheckpointMoment::BeforeTurn);
    assert_eq!(edited.after.moment, CheckpointMoment::AfterTurn);
    assert_eq!(edited.before.files, 3, "a.txt, b.txt, and .gitignore");

    // A command changes a file no edit named, and one git ignores.
    let command = format!(
        "run {} && {} && {}",
        overwrite("b.txt", "by-a-command"),
        overwrite("c.txt", "new"),
        overwrite("ignored.txt", "x")
    );
    let ran = turn(&mut client, conversation, &command).await;
    let ran = changes(&mut client, conversation, ran).await;
    let listed: Vec<(&str, FileChangeKind)> = ran
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("b.txt", FileChangeKind::Changed),
            ("c.txt", FileChangeKind::Added),
        ]
    );
}

#[tokio::test]
async fn a_turn_before_which_nothing_changed_stores_no_content() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    let first = turn(&mut client, conversation, "say first").await;
    let stored = objects(&daemon);
    assert_eq!(stored, 1, "the content is stored once, by its hash");
    assert_eq!(
        changes(&mut client, conversation, first)
            .await
            .before
            .stored_bytes,
        3
    );
    let second = turn(&mut client, conversation, "say second").await;
    let second = changes(&mut client, conversation, second).await;
    assert_eq!(second.before.stored_bytes, 0);
    assert!(second.changes.is_empty());
    assert_eq!(objects(&daemon), stored);
}

#[tokio::test]
async fn a_record_that_cannot_be_stored_lets_the_turn_run_and_says_why() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    // A file where the contents' folder goes.
    std::fs::write(daemon.data_dir().join("checkpoints"), "in the way").unwrap();
    client.say(conversation, "say still here").await;
    let skipped = client
        .until_event(|event| matches!(event, Event::CheckpointSkipped { .. }))
        .await;
    let Event::CheckpointSkipped { moment, cause, .. } = skipped else {
        unreachable!()
    };
    assert_eq!(
        (moment, cause),
        (CheckpointMoment::BeforeTurn, CheckpointCause::Storage)
    );
    client.until_reply("still here").await;
    client.until_turn_ends().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_file_lets_the_turn_run_and_is_named() {
    use std::os::unix::fs::PermissionsExt;

    if crate::support::skipped_as_root("root reads a file of mode 000") {
        return;
    }
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("locked.txt"), "locked").unwrap();
    std::fs::set_permissions(
        folder.join("locked.txt"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    client.say(conversation, "say still here").await;
    let skipped = client
        .until_event(|event| matches!(event, Event::CheckpointSkipped { .. }))
        .await;
    let Event::CheckpointSkipped { cause, path, .. } = skipped else {
        unreachable!()
    };
    assert_eq!(cause, CheckpointCause::Unreadable);
    assert_eq!(path.as_deref(), Some("locked.txt"));
    client.until_reply("still here").await;
}

#[tokio::test]
async fn deleting_the_history_removes_the_contents_no_record_names() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    turn(&mut client, conversation, "edit a.txt two").await;
    assert_eq!(objects(&daemon), 2);
    client.send(ClientMessage::DeleteHistory { bot_id }).await;
    client
        .until(|message| matches!(message, ServerMessage::HistoryDeleted { .. }).then_some(()))
        .await;
    assert!(
        eventually(Duration::from_secs(10), || objects(&daemon) == 0),
        "the contents are gone"
    );
    assert_eq!(read(&folder, "a.txt"), "two", "the folder itself stays");
}

#[tokio::test]
async fn a_restore_puts_back_what_an_edit_and_a_command_changed() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    std::fs::write(folder.join("b.txt"), "two").unwrap();
    std::fs::write(
        folder.join("big.bin"),
        vec![b'x'; CHECKPOINT_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let first = turn(&mut client, conversation, "edit a.txt by an edit").await;
    let command = format!(
        "run {} && {} && {}",
        overwrite("b.txt", "by-a-command"),
        overwrite("added.txt", "new"),
        overwrite("big.bin", "small")
    );
    turn(&mut client, conversation, &command).await;

    // Back to before the first turn, all of it.
    let Restore::Applied(Event::CheckpointRestored {
        turn_event_id,
        restored,
        removed,
        left,
    }) = restore(&mut client, conversation, first, None, false).await
    else {
        panic!("the restore was refused")
    };
    assert_eq!(turn_event_id, first);
    assert_eq!(restored, ["a.txt", "b.txt"]);
    assert_eq!(removed, ["added.txt"]);
    assert_eq!(read(&folder, "a.txt"), "one");
    assert_eq!(read(&folder, "b.txt"), "two");
    assert!(!folder.join("added.txt").exists());
    // The large file kept no content: it stays as it is, and is named.
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].path.as_str(), left[0].cause),
        ("big.bin", LeftCause::NotKept)
    );
    assert!(read(&folder, "big.bin").starts_with("small"));

    // The event is in the conversation, and the folder as it stood before
    // the restore was recorded first.
    let snapshot = client.open(conversation).await;
    assert!(
        snapshot
            .events
            .iter()
            .any(|timed| matches!(timed.event, Event::CheckpointRestored { .. }))
    );
    let connection = rusqlite::Connection::open(daemon.database()).unwrap();
    let moments: Vec<String> = connection
        .prepare("SELECT moment FROM checkpoints ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        moments,
        [
            "before_turn",
            "after_turn",
            "before_turn",
            "after_turn",
            "before_restore",
            "after_restore"
        ]
    );
}

#[tokio::test]
async fn a_restore_of_named_paths_covers_those_alone() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    std::fs::write(folder.join("b.txt"), "two").unwrap();
    let first = turn(&mut client, conversation, "edit a.txt changed").await;
    turn(&mut client, conversation, "edit b.txt changed").await;
    let Restore::Applied(Event::CheckpointRestored { restored, .. }) = restore(
        &mut client,
        conversation,
        first,
        Some(vec!["b.txt".to_owned()]),
        false,
    )
    .await
    else {
        panic!("the restore was refused")
    };
    assert_eq!(restored, ["b.txt"]);
    assert_eq!(read(&folder, "b.txt"), "two");
    assert_eq!(read(&folder, "a.txt"), "changed");
}

#[tokio::test]
async fn a_file_another_hand_changed_is_written_over_only_when_confirmed() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (_, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    let first = turn(&mut client, conversation, "edit a.txt by the turn").await;
    std::fs::write(folder.join("a.txt"), "by a person, after the turn").unwrap();
    let Restore::Refused(cause, conflicts) =
        restore(&mut client, conversation, first, None, false).await
    else {
        panic!("the restore wrote over the person's change")
    };
    assert_eq!(cause, RestoreRefusal::Conflicts);
    assert_eq!(conflicts, ["a.txt"]);
    assert_eq!(read(&folder, "a.txt"), "by a person, after the turn");
    let Restore::Applied(_) = restore(&mut client, conversation, first, None, true).await else {
        panic!("the confirmed restore was refused")
    };
    assert_eq!(read(&folder, "a.txt"), "one");
}

#[tokio::test]
async fn a_restore_is_refused_while_a_turn_of_the_bot_runs() {
    let daemon = Daemon::start();
    let mut client = daemon.client().await;
    let (bot_id, conversation, folder) = bot(&daemon, &mut client).await;
    std::fs::write(folder.join("a.txt"), "one").unwrap();
    let first = turn(&mut client, conversation, "edit a.txt changed").await;
    // Another conversation of the bot runs a turn that waits.
    client.send(ClientMessage::NewConversation { bot_id }).await;
    let other = client
        .until(|message| match message {
            ServerMessage::ConversationCreated(summary) => Some(summary.id),
            _ => None,
        })
        .await;
    client.open(other).await;
    client.say(other, "ponder a while").await;
    client
        .until_event(|event| matches!(event, Event::ThoughtText(_)))
        .await;
    let Restore::Refused(cause, _) = restore(&mut client, conversation, first, None, false).await
    else {
        panic!("the restore ran during a turn")
    };
    assert_eq!(cause, RestoreRefusal::TurnRunning);
    assert_eq!(read(&folder, "a.txt"), "changed");
    client
        .send(ClientMessage::StopTurn {
            conversation_id: other,
        })
        .await;
    client.until_turn_ends().await;
    // The bot shows as idle once its conversations told the daemon so.
    client
        .until_bot(|summary| summary.bot.id == bot_id && summary.state != BotState::Working)
        .await;
    let Restore::Applied(_) = restore(&mut client, conversation, first, None, false).await else {
        panic!("the restore was refused once the turn ended")
    };
    assert_eq!(read(&folder, "a.txt"), "one");
}
