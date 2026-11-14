use serde::{Deserialize, Serialize};

use crate::bot::{BotDraft, BotPatch, BotSummary, present};
use crate::event::{ApprovalDecision, Event};
use crate::preferences::{Preferences, PreferencesPatch};

pub const PROTOCOL_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
    },
    ListBots,
    CreateBot {
        draft: BotDraft,
    },
    UpdateBot {
        bot_id: i64,
        patch: BotPatch,
    },
    /// Creates a bot with the settings of another one and no conversation of it.
    DuplicateBot {
        bot_id: i64,
    },
    /// Stops the bot's running turns, then removes it and its conversations.
    DeleteBot {
        bot_id: i64,
    },
    /// Asks for the conversations of a bot, the most recently active first.
    ListConversations {
        bot_id: i64,
    },
    /// Starts a conversation and makes it the bot's current one.
    NewConversation {
        bot_id: i64,
    },
    OpenConversation {
        conversation_id: i64,
    },
    CloseConversation {
        conversation_id: i64,
    },
    SendMessage {
        conversation_id: i64,
        text: String,
    },
    ResolveApproval {
        conversation_id: i64,
        approval_id: String,
        decision: ApprovalDecision,
    },
    StopTurn {
        conversation_id: i64,
    },
    EditQueuedMessage {
        conversation_id: i64,
        queue_id: i64,
        text: String,
    },
    DeleteQueuedMessage {
        conversation_id: i64,
        queue_id: i64,
    },
    SendQueuedMessageNow {
        conversation_id: i64,
        queue_id: i64,
    },
    ResumeQueue {
        conversation_id: i64,
    },
    /// The client shows the conversation up to its end: all of it is read.
    MarkRead {
        conversation_id: i64,
        /// The client keeps the end on screen: what arrives next is read
        /// too, until it sends a mark that says otherwise or closes the
        /// conversation.
        #[serde(default)]
        following: bool,
    },
    MarkBotRead {
        bot_id: i64,
    },
    /// Moves the read mark back, before the last turn that ended.
    MarkBotUnread {
        bot_id: i64,
    },
    GetPreferences,
    SetPreferences {
        patch: PreferencesPatch,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        daemon_version: String,
    },
    Bots(Vec<BotSummary>),
    /// A bot was created or changed. Every attached client receives it.
    BotChanged(BotSummary),
    /// Answers the client that created or duplicated a bot, after `BotChanged`.
    BotCreated {
        bot_id: i64,
    },
    /// A bot and its conversations are gone. Every attached client receives it.
    BotDeleted {
        bot_id: i64,
    },
    Conversations {
        bot_id: i64,
        conversations: Vec<ConversationSummary>,
    },
    /// Answers the client that started a conversation.
    ConversationCreated(ConversationSummary),
    /// A conversation was created or got its title. Every attached client receives it.
    ConversationChanged(ConversationSummary),
    ConversationOpened(ConversationSnapshot),
    ConversationEvent {
        conversation_id: i64,
        event: Event,
    },
    QueueChanged {
        conversation_id: i64,
        queue: QueueState,
    },
    Error {
        message: String,
    },
    /// A request was refused because of one of its fields, and nothing was stored.
    InvalidField {
        field: String,
        message: String,
    },
    Preferences(Preferences),
    Incompatible {
        protocol_version: u32,
        daemon_version: String,
    },
    TurnModel {
        conversation_id: i64,
        model: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: i64,
    pub bot_id: i64,
    pub title: Option<String>,
    pub created_at_ms: i64,
    pub last_activity_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation: ConversationSummary,
    pub events: Vec<Event>,
    pub turn_active: bool,
    pub pending_approvals: Vec<String>,
    #[serde(default)]
    pub queue: QueueState,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub turn_model: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    pub messages: Vec<QueuedMessage>,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: i64,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(turn_model: Option<Option<String>>) -> ConversationSnapshot {
        ConversationSnapshot {
            conversation: ConversationSummary {
                id: 1,
                bot_id: 1,
                title: None,
                created_at_ms: 0,
                last_activity_ms: 0,
            },
            events: Vec::new(),
            turn_active: turn_model.is_some(),
            pending_approvals: Vec::new(),
            queue: QueueState::default(),
            turn_model,
        }
    }

    #[test]
    fn a_snapshot_tells_the_default_model_of_a_turn_from_no_turn() {
        for turn_model in [None, Some(None), Some(Some("opus".to_owned()))] {
            let sent = snapshot(turn_model);
            let wire = serde_json::to_string(&sent).unwrap();
            assert_eq!(
                serde_json::from_str::<ConversationSnapshot>(&wire).unwrap(),
                sent
            );
        }
    }

    #[test]
    fn a_snapshot_from_an_older_daemon_has_no_turn_model() {
        let wire = r#"{"conversation":{"id":1,"bot_id":1,"title":null,"created_at_ms":0,
            "last_activity_ms":0},"events":[],"turn_active":true,"pending_approvals":[]}"#;
        let read: ConversationSnapshot = serde_json::from_str(wire).unwrap();
        assert_eq!(read.turn_model, None);
        assert!(read.turn_active);
    }
}
