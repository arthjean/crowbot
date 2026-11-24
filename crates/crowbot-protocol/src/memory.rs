use serde::{Deserialize, Serialize};

/// The most characters the entries flagged `core` of one bot's `Bot` scope
/// may hold together.
pub const CORE_MEMORY_BOT_CHARACTERS: usize = 2_200;
/// The most characters the entries flagged `core` of the `User` scope may
/// hold together.
pub const CORE_MEMORY_USER_CHARACTERS: usize = 1_375;
/// The longest text one memory entry may hold, in characters.
pub const MEMORY_ENTRY_CHARACTERS: usize = 2_200;
/// The most operations one change of memory may hold.
pub const MEMORY_OPERATIONS: usize = 32;

/// Whose fact an entry is: the bot's own, or the user's, which every bot
/// reads and which outlives the bot that wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Bot,
    User,
}

impl MemoryScope {
    /// The most characters the `core` entries of the scope may hold.
    pub fn core_limit(self) -> usize {
        match self {
            Self::Bot => CORE_MEMORY_BOT_CHARACTERS,
            Self::User => CORE_MEMORY_USER_CHARACTERS,
        }
    }
}

/// Who wrote an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAuthor {
    /// A bot, in a turn of its own conversation.
    Bot,
    /// A bot reviewing its work.
    Review,
    /// A person, through a client.
    User,
}

/// One active memory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub scope: MemoryScope,
    /// The bot of a `Bot` entry; `None` for a `User` entry.
    pub bot_id: Option<i64>,
    pub text: String,
    /// Whether the entry is in the system prompt of every session, or only
    /// recalled by a turn whose message matches it.
    pub core: bool,
    pub author: MemoryAuthor,
    /// The event that grounds the entry, and its conversation. A person's
    /// entry may name none; the event may be gone with its history.
    pub source_event_id: Option<i64>,
    #[serde(default)]
    pub source_conversation_id: Option<i64>,
    /// The entry this one corrected.
    #[serde(default)]
    pub replaces: Option<i64>,
    pub created_at_ms: i64,
    /// What the scan finds in the text now. An entry with a finding stays
    /// in the list and is replaced by a placeholder in the prompt.
    #[serde(default)]
    pub findings: Vec<ScanFinding>,
}

/// One operation of a change of memory, as a client or a bot asks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryOperation {
    Add {
        scope: MemoryScope,
        text: String,
        #[serde(default)]
        core: bool,
        /// The event that grounds the fact. A bot's entry with none is
        /// grounded by the message that started its turn.
        #[serde(default)]
        source_event_id: Option<i64>,
    },
    /// Replaces the text or the `core` flag of an entry: the entry is
    /// retired, and a new one that names it takes its place.
    Correct {
        id: i64,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        core: Option<bool>,
    },
    Retire {
        id: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryChangeKind {
    Added,
    Corrected,
    Retired,
}

/// What one operation did, as `MemoryChanged` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChange {
    pub kind: MemoryChangeKind,
    /// The entry added, the new entry of a correction, or the entry retired.
    pub entry_id: i64,
    /// The entry a correction retired.
    #[serde(default)]
    pub replaced_id: Option<i64>,
    pub scope: MemoryScope,
    pub text: String,
    #[serde(default)]
    pub core: bool,
}

/// What a text the daemon injects into a prompt tries to make a bot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanCategory {
    /// Replaces the instructions the bot was given.
    InstructionOverride,
    /// Hides something from the user, or hides text from a reader.
    Concealment,
    /// Reads credentials or secrets.
    SecretReading,
    /// Sends data out of the computer.
    Exfiltration,
    /// Keeps access, or changes what the agent loads next time.
    Persistence,
    /// Destroys data.
    Destruction,
    #[serde(other)]
    Other,
}

impl ScanCategory {
    /// The words the daemon names the category with.
    pub fn label(self) -> &'static str {
        match self {
            Self::InstructionOverride => "instruction override",
            Self::Concealment => "concealment from the user",
            Self::SecretReading => "secret reading",
            Self::Exfiltration => "exfiltration",
            Self::Persistence => "persistence",
            Self::Destruction => "destruction",
            Self::Other => "unknown category",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanSeverity {
    Caution,
    Dangerous,
}

/// One thing the scan found in a text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFinding {
    pub category: ScanCategory,
    pub severity: ScanSeverity,
    /// The name of the pattern that matched.
    pub pattern: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_names_itself_and_takes_defaults() {
        let read: MemoryOperation =
            serde_json::from_str(r#"{"op":"add","scope":"user","text":"Likes tea"}"#).unwrap();
        assert_eq!(
            read,
            MemoryOperation::Add {
                scope: MemoryScope::User,
                text: "Likes tea".to_owned(),
                core: false,
                source_event_id: None,
            }
        );
        let wire = serde_json::to_string(&MemoryOperation::Retire { id: 4 }).unwrap();
        assert_eq!(wire, r#"{"op":"retire","id":4}"#);
        assert_eq!(MemoryScope::Bot.core_limit(), 2_200);
        assert_eq!(MemoryScope::User.core_limit(), 1_375);
    }
}
