use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
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
    ApprovalRequested {
        id: String,
        tool_call_id: String,
        title: Option<String>,
    },
    ApprovalResolved {
        #[serde(default)]
        id: Option<String>,
        tool_call_id: String,
        title: Option<String>,
        decision: ApprovalDecision,
    },
    TurnEnded(TurnEnd),
    TurnFailed {
        message: String,
    },
}

impl Event {
    pub fn ends_turn(&self) -> bool {
        matches!(self, Self::TurnEnded(_) | Self::TurnFailed { .. })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_resolved_by_an_older_daemon_still_loads() {
        let stored = r#"{"type":"approval_resolved","data":{"tool_call_id":"t1","title":"Write","decision":"declined"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(stored).unwrap(),
            Event::ApprovalResolved {
                id: None,
                tool_call_id: "t1".to_owned(),
                title: Some("Write".to_owned()),
                decision: ApprovalDecision::Declined,
            }
        );
    }

    #[test]
    fn stored_text_events_keep_their_shape() {
        assert_eq!(
            serde_json::to_string(&Event::MessageText("caw".to_owned())).unwrap(),
            r#"{"type":"message_text","data":"caw"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::TurnEnded(TurnEnd::Completed)).unwrap(),
            r#"{"type":"turn_ended","data":"completed"}"#
        );
    }

    #[test]
    fn only_turn_ends_and_failures_end_a_turn() {
        assert!(Event::TurnEnded(TurnEnd::Cancelled).ends_turn());
        assert!(
            Event::TurnFailed {
                message: "gone".to_owned()
            }
            .ends_turn()
        );
        assert!(!Event::MessageText("caw".to_owned()).ends_turn());
    }
}
