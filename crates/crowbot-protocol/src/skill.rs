use serde::{Deserialize, Serialize};

use crate::memory::ScanFinding;

/// The longest name of a skill, in characters.
pub const SKILL_NAME_CHARACTERS: usize = 64;
/// The longest description a skill's front matter may hold, in characters.
pub const SKILL_DESCRIPTION_CHARACTERS: usize = 1_024;
/// The longest `SKILL.md`, in characters.
pub const SKILL_DOCUMENT_CHARACTERS: usize = 100_000;
/// The largest file of a skill, in bytes.
pub const SKILL_FILE_BYTES: usize = 1 << 20;
/// The most files one version of a skill may hold, its `SKILL.md` included.
pub const SKILL_FILES: usize = 256;
/// The most operations one change of skills may hold.
pub const SKILL_OPERATIONS: usize = 32;
/// The longest description a line of the skills index shows, in characters.
pub const SKILL_INDEX_DESCRIPTION_CHARACTERS: usize = 60;
/// The file every skill holds, at the root of its folder.
pub const SKILL_DOCUMENT: &str = "SKILL.md";
/// The folders of a skill that may hold its other files.
pub const SKILL_FOLDERS: [&str; 4] = ["references", "templates", "scripts", "assets"];

/// Who made a version of a skill, or changed its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuthor {
    /// A bot, in a turn of its own conversation.
    Bot,
    /// A bot reviewing its work.
    Review,
    /// A person, through a client.
    User,
}

/// Whether a skill is in the index of the bot's sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    Active,
    /// Set aside: out of the index and of the projection, its versions kept.
    Archived,
}

/// A skill of a bot, as a client lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub state: SkillState,
    /// A pinned skill is not archived by a bot.
    pub pinned: bool,
    /// The SHA-256 of the active version.
    pub hash: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// One version a skill had, as its history lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillVersion {
    pub hash: String,
    pub created_at_ms: i64,
    pub author: SkillAuthor,
    /// The conversation whose turn or client message made it.
    pub conversation_id: Option<i64>,
    pub kind: SkillChangeKind,
    /// Whether it is the active version now.
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillChangeKind {
    Created,
    /// A new version of a skill that existed.
    Corrected,
    Archived,
    Restored,
    /// A person made an earlier version active again.
    RolledBack,
    /// A person gave the bot a skill from a folder.
    Imported,
    Pinned,
    Unpinned,
    #[serde(other)]
    Other,
}

/// What one call or one client message did to one skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillChange {
    pub name: String,
    pub kind: SkillChangeKind,
    /// The active version before the change; none for a new skill.
    #[serde(default)]
    pub old_hash: Option<String>,
    /// The active version after it.
    #[serde(default)]
    pub new_hash: Option<String>,
}

/// A change of a skill's state that a person asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStateChange {
    Pin,
    Unpin,
    Archive,
    Restore,
}

/// What the scan found in one file of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFinding {
    /// The file's path in the skill's folder.
    pub path: String,
    pub finding: ScanFinding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_reads_without_its_hashes() {
        let change: SkillChange =
            serde_json::from_str(r#"{"name":"deploy","kind":"archived"}"#).unwrap();
        assert_eq!(change.old_hash, None);
        assert_eq!(change.kind, SkillChangeKind::Archived);
        let unknown: SkillChangeKind = serde_json::from_str(r#""merged""#).unwrap();
        assert_eq!(unknown, SkillChangeKind::Other);
    }
}
