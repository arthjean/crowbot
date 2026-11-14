use serde::{Deserialize, Serialize};

use crate::bot::{BotDraft, BotPatch, BotSummary};
use crate::event::{ApprovalDecision, Event};

pub const PROTOCOL_VERSION: u32 = 3;

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
