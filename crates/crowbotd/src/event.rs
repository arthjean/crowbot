use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    SessionStarted {
        provider_session_id: String,
    },
    UserText(String),
    MessageText(String),
    ThoughtText(String),
    ToolCallStarted {
        id: String,
        title: String,
        status: Option<ToolStatus>,
    },
    ToolCallUpdated {
        id: String,
        title: Option<String>,
        status: Option<ToolStatus>,
    },
    ApprovalResolved {
        tool_call_id: String,
        title: Option<String>,
        decision: ApprovalDecision,
    },
    TurnEnded(TurnEnd),
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session_started",
            Self::UserText(_) => "user_text",
            Self::MessageText(_) => "message_text",
            Self::ThoughtText(_) => "thought_text",
            Self::ToolCallStarted { .. } => "tool_call_started",
            Self::ToolCallUpdated { .. } => "tool_call_updated",
            Self::ApprovalResolved { .. } => "approval_resolved",
            Self::TurnEnded(_) => "turn_ended",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Decline,
    AllowOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allowed,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    MaxTokens,
    MaxRequests,
    Refused,
    Cancelled,
    Other,
}
