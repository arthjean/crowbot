use serde::{Deserialize, Deserializer, Serialize};

/// The longest name a bot can have, in characters.
pub const BOT_NAME_LENGTH: usize = 80;
/// The longest title a bot can have, in characters.
pub const BOT_TITLE_LENGTH: usize = 80;
/// The longest instructions a bot can have, in characters.
pub const BOT_INSTRUCTIONS_LENGTH: usize = 20_000;
/// The longest model name a bot can have, in characters.
pub const BOT_MODEL_LENGTH: usize = 200;
/// The colors an avatar is picked from, as `#rrggbb`. A bot created with no
/// avatar takes the next one of this list.
pub const AVATAR_COLORS: [&str; 8] = [
    "#3b82f6", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899", "#14b8a6", "#f97316",
];

/// What a bot may do without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Every gated action waits for a person.
    #[default]
    AskFirst,
    /// The daemon allows each gated action once, on its own.
    FullAccess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Avatar {
    /// A color as `#rrggbb`.
    pub color: String,
    /// A letter or an emoji.
    pub glyph: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    pub id: i64,
    pub name: String,
    pub title: String,
    pub instructions: String,
    pub avatar: Avatar,
    pub provider: String,
    /// `None` means the provider's default model.
    pub model: Option<String>,
    pub permission_level: PermissionLevel,
    /// The folder the bot works in.
    pub workspace: String,
    pub pinned: bool,
    pub hidden: bool,
    pub created_at_ms: i64,
}

/// What a client gives to create a bot. Only the name is required: the daemon
/// chooses the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotDraft {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub avatar: Option<Avatar>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_level: Option<PermissionLevel>,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// The fields of a bot that a client changes. A field left out keeps its value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Avatar>,
    /// `Some(None)` goes back to the provider's default model.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_level: Option<PermissionLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// Reads a field that is present, so that `null` differs from a field left out.
pub(crate) fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// What a bot is doing, across all its conversations. When several apply, the
/// one a person must act on first wins: waiting, then failed, then working,
/// then unread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotState {
    Idle,
    /// A turn ended since the last read mark.
    Unread,
    /// A turn is running.
    Working,
    /// The last turn of a conversation failed or was interrupted, and nobody
    /// has seen it yet.
    Failed,
    /// An approval waits for a person.
    Waiting,
}

impl BotState {
    pub fn of(waiting: bool, failed: bool, working: bool, unread: u32) -> Self {
        if waiting {
            Self::Waiting
        } else if failed {
            Self::Failed
        } else if working {
            Self::Working
        } else if unread > 0 {
            Self::Unread
        } else {
            Self::Idle
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSummary {
    pub bot: Bot,
    /// The conversation a client shows when the bot is opened.
    pub current_conversation_id: i64,
    pub state: BotState,
    /// The turns that ended since the last read mark, over all conversations.
    pub unread: u32,
    pub last_activity_ms: i64,
    #[serde(default)]
    pub preview: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_tells_a_cleared_model_from_an_untouched_one() {
        let untouched: BotPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(untouched.model, None);
        let cleared: BotPatch = serde_json::from_str(r#"{"model":null}"#).unwrap();
        assert_eq!(cleared.model, Some(None));
        let chosen: BotPatch = serde_json::from_str(r#"{"model":"opus"}"#).unwrap();
        assert_eq!(chosen.model, Some(Some("opus".to_owned())));

        for patch in [untouched, cleared, chosen] {
            let wire = serde_json::to_string(&patch).unwrap();
            assert_eq!(serde_json::from_str::<BotPatch>(&wire).unwrap(), patch);
        }
    }

    #[test]
    fn a_draft_needs_only_a_name_and_a_new_bot_asks_first() {
        let draft: BotDraft = serde_json::from_str(r#"{"name":"Atlas"}"#).unwrap();
        assert_eq!(draft.name, "Atlas");
        assert_eq!(draft.permission_level, None);
        assert_eq!(PermissionLevel::default(), PermissionLevel::AskFirst);
    }

    #[test]
    fn the_state_a_person_must_act_on_first_wins() {
        assert_eq!(BotState::of(true, true, true, 3), BotState::Waiting);
        assert_eq!(BotState::of(false, true, true, 3), BotState::Failed);
        assert_eq!(BotState::of(false, false, true, 3), BotState::Working);
        assert_eq!(BotState::of(false, false, false, 3), BotState::Unread);
        assert_eq!(BotState::of(false, false, false, 0), BotState::Idle);
    }
}
