#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allowed,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    Completed,
    MaxTokens,
    MaxRequests,
    Refused,
    Cancelled,
    Other,
}
