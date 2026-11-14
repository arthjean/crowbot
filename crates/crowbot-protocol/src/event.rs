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
        /// Why the daemon answered on its own. `None` means a person answered.
        #[serde(default)]
        reason: Option<ApprovalReason>,
    },
    ApprovalClosed {
        id: String,
        tool_call_id: String,
        title: Option<String>,
        reason: ApprovalClosure,
    },
    TurnEnded(TurnEnd),
    TurnFailed {
        message: String,
    },
    TurnInterrupted {
        cause: Interruption,
    },
}

impl Event {
    pub fn ends_turn(&self) -> bool {
        matches!(
            self,
            Self::TurnEnded(_) | Self::TurnFailed { .. } | Self::TurnInterrupted { .. }
        )
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

/// Why the daemon answered an approval request without a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReason {
    /// The bot is on Full access.
    PermissionLevel,
    /// The request offered no allow-once option, so it could only be declined.
    NoAllowOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalClosure {
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interruption {
    DaemonRestarted,
    AgentUnresponsive,
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
                reason: None,
            }
        );
    }

    #[test]
    fn an_approval_answered_by_the_permission_level_has_a_stable_stored_shape() {
        assert_eq!(
            serde_json::to_string(&Event::ApprovalResolved {
                id: Some("a1".to_owned()),
                tool_call_id: "t1".to_owned(),
                title: None,
                decision: ApprovalDecision::Allowed,
                reason: Some(ApprovalReason::PermissionLevel),
            })
            .unwrap(),
            r#"{"type":"approval_resolved","data":{"id":"a1","tool_call_id":"t1","title":null,"decision":"allowed","reason":"permission_level"}}"#
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
    fn an_interruption_and_a_closed_approval_have_a_stable_stored_shape() {
        assert_eq!(
            serde_json::to_string(&Event::TurnInterrupted {
                cause: Interruption::DaemonRestarted
            })
            .unwrap(),
            r#"{"type":"turn_interrupted","data":{"cause":"daemon_restarted"}}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::ApprovalClosed {
                id: "a1".to_owned(),
                tool_call_id: "t1".to_owned(),
                title: None,
                reason: ApprovalClosure::Expired,
            })
            .unwrap(),
            r#"{"type":"approval_closed","data":{"id":"a1","tool_call_id":"t1","title":null,"reason":"expired"}}"#
        );
    }

    #[test]
    fn turn_ends_failures_and_interruptions_end_a_turn() {
        assert!(Event::TurnEnded(TurnEnd::Cancelled).ends_turn());
        assert!(
            Event::TurnFailed {
                message: "gone".to_owned()
            }
            .ends_turn()
        );
        assert!(
            Event::TurnInterrupted {
                cause: Interruption::AgentUnresponsive
            }
            .ends_turn()
        );
        assert!(!Event::MessageText("caw".to_owned()).ends_turn());
    }
}
