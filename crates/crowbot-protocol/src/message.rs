use serde::{Deserialize, Serialize};

use crate::event::{ApprovalDecision, Event};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
    },
    ListConversations,
    CreateConversation,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        daemon_version: String,
    },
    Conversations(Vec<ConversationSummary>),
    ConversationCreated(ConversationSummary),
    ConversationOpened(ConversationSnapshot),
    ConversationEvent {
        conversation_id: i64,
        event: Event,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: i64,
    pub title: Option<String>,
    pub cwd: String,
    pub created_at_ms: i64,
    pub last_activity_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation: ConversationSummary,
    pub events: Vec<Event>,
    pub turn_active: bool,
    pub pending_approvals: Vec<String>,
}
