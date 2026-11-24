//! Checkpoints: the daemon records the bot's folder before and after each
//! turn, so that what a turn changed can be read and put back, whatever
//! changed it (EP-008 of the harness PRD).

use serde::{Deserialize, Serialize};

/// The most paths one restore names.
pub const RESTORE_PATHS: usize = 1_000;

/// When the daemon records the bot's folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointMoment {
    /// Before the prompt of a turn reaches the agent.
    BeforeTurn,
    /// Once the end of a turn is recorded.
    AfterTurn,
    /// Just before a restore writes the folder.
    BeforeRestore,
    /// Once a restore wrote the folder.
    AfterRestore,
    #[serde(other)]
    Other,
}

/// Why a record of the folder could not be completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointCause {
    /// A file or a folder of the bot's folder could not be read.
    Unreadable,
    /// What the daemon keeps could not be written.
    Storage,
    /// The folder holds more entries than a record takes.
    TooManyFiles,
    /// The bot's folder does not exist.
    FolderMissing,
    #[serde(other)]
    Other,
}

/// What an entry of the folder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    /// A symbolic link, recorded with its target and never followed.
    Link,
}

/// An entry of the folder as a record holds it. For a link, `size` and
/// `sha256` are those of its target's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub kind: FileKind,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
    /// The daemon kept the content, so a restore can write it back. A file
    /// over the size a record keeps has its hash alone.
    pub kept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Changed,
    Removed,
}

/// One file a turn added, changed, or removed, by its path relative to the
/// bot's folder, with `/` between its parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    #[serde(default)]
    pub before: Option<FileState>,
    #[serde(default)]
    pub after: Option<FileState>,
}

/// A record of the folder: when it was taken, how long it took, and what
/// it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointSummary {
    pub moment: CheckpointMoment,
    pub taken_at_ms: i64,
    pub duration_ms: u64,
    pub files: u64,
    pub bytes: u64,
    /// The bytes of content this record stored that no earlier one had.
    pub stored_bytes: u64,
}

/// What a turn changed in the bot's folder: the difference between the
/// records taken before and after it. Answers `GetTurnChanges`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnChanges {
    pub conversation_id: i64,
    /// The `UserText` event that started the turn.
    pub event_id: i64,
    pub before: CheckpointSummary,
    pub after: CheckpointSummary,
    pub changes: Vec<FileChange>,
}

/// Why a restore left a file as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftCause {
    /// The record holds the file's hash, not its content.
    NotKept,
    /// The content the record names is gone or no longer matches its hash.
    ContentLost,
    /// The file could not be written or removed.
    Failed,
    /// The record left the file out, as a `.gitignore` of the folder matched
    /// it then: the file may have been there before the turn, so a restore
    /// does not remove it.
    Ignored,
    #[serde(other)]
    Other,
}

/// A file a restore covered and left as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftFile {
    pub path: String,
    pub cause: LeftCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Why a restore was refused. Nothing of the folder was written, except
/// for `Other` when its message says the files were put back and the record
/// of the restore failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreRefusal {
    /// A turn runs in a conversation of a bot of that folder.
    TurnRunning,
    /// Files changed since the last record, by another hand than a turn,
    /// or files whose content no record kept (git ignores them now, or they
    /// are too large to keep): the restore runs only when the message
    /// confirms it.
    Conflicts,
    /// No record of the folder was taken before that turn.
    NoCheckpoint,
    /// The bot works in another folder than the one the turn ran in.
    FolderChanged,
    /// A path the message names is not a path inside the folder.
    InvalidPath,
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use crate::Event;

    #[test]
    fn a_restore_event_reads_with_no_files_left() {
        let read: Event = serde_json::from_str(
            r#"{"type":"checkpoint_restored","data":{"turn_event_id":4,"restored":["a.txt"]}}"#,
        )
        .unwrap();
        assert_eq!(
            read,
            Event::CheckpointRestored {
                turn_event_id: 4,
                restored: vec!["a.txt".to_owned()],
                removed: Vec::new(),
                left: Vec::new(),
            }
        );
        let wire = serde_json::to_string(&read).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&wire).unwrap(), read);
    }

    #[test]
    fn a_cause_this_client_does_not_know_reads_as_other() {
        let read: Event = serde_json::from_str(
            r#"{"type":"checkpoint_skipped","data":{"moment":"later","cause":"new","message":"m"}}"#,
        )
        .unwrap();
        assert_eq!(
            read,
            Event::CheckpointSkipped {
                moment: super::CheckpointMoment::Other,
                cause: super::CheckpointCause::Other,
                path: None,
                message: "m".to_owned(),
            }
        );
    }
}
