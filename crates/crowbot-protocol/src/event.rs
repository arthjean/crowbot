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
        #[serde(default)]
        kind: Option<ToolKind>,
        #[serde(default)]
        input: Option<String>,
    },
    ToolCallUpdated {
        id: String,
        title: Option<String>,
        status: Option<ToolStatus>,
        #[serde(default)]
        kind: Option<ToolKind>,
        #[serde(default)]
        input: Option<String>,
        #[serde(default)]
        output: Option<String>,
    },
    ApprovalRequested {
        id: String,
        tool_call_id: String,
        title: Option<String>,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        cut: bool,
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
        #[serde(default)]
        cause: Option<FailureCause>,
        #[serde(default)]
        provider: Option<String>,
    },
    TurnInterrupted {
        cause: Interruption,
    },
    /// The bot's model is not one the provider offers, so the turn runs on
    /// the provider's default model.
    ModelUnavailable {
        model: String,
        #[serde(default)]
        provider: Option<String>,
    },
    /// The bot moved to another provider. The turn that follows runs on `to`,
    /// which received what was said before.
    ProviderChanged {
        from: String,
        to: String,
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

pub const TOOL_OUTPUT_BYTES: usize = 64 << 10;
pub const TOOL_INPUT_BYTES: usize = 2 << 10;
pub const APPROVAL_TARGET_BYTES: usize = 16 << 10;

pub fn cut_text(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    SignInRequired,
    UsageLimit,
    #[serde(other)]
    Other,
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
    fn a_model_fallback_and_a_provider_change_have_a_stable_stored_shape() {
        let fallback = Event::ModelUnavailable {
            model: "opus".to_owned(),
            provider: Some("claude".to_owned()),
        };
        assert_eq!(
            serde_json::to_string(&fallback).unwrap(),
            r#"{"type":"model_unavailable","data":{"model":"opus","provider":"claude"}}"#
        );
        assert!(!fallback.ends_turn());
        let moved = Event::ProviderChanged {
            from: "claude".to_owned(),
            to: "codex".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&moved).unwrap(),
            r#"{"type":"provider_changed","data":{"from":"claude","to":"codex"}}"#
        );
        assert!(!moved.ends_turn());
    }

    #[test]
    fn turn_ends_failures_and_interruptions_end_a_turn() {
        assert!(Event::TurnEnded(TurnEnd::Cancelled).ends_turn());
        assert!(
            Event::TurnFailed {
                message: "gone".to_owned(),
                cause: None,
                provider: None,
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

    #[test]
    fn events_stored_before_tool_details_existed_still_load() {
        let started =
            r#"{"type":"tool_call_started","data":{"id":"t1","title":"Read","status":"pending"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(started).unwrap(),
            Event::ToolCallStarted {
                id: "t1".to_owned(),
                title: "Read".to_owned(),
                status: Some(ToolStatus::Pending),
                kind: None,
                input: None,
            }
        );
        let updated =
            r#"{"type":"tool_call_updated","data":{"id":"t1","title":null,"status":"completed"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(updated).unwrap(),
            Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: Some(ToolStatus::Completed),
                kind: None,
                input: None,
                output: None,
            }
        );
        let requested = r#"{"type":"approval_requested","data":{"id":"a1","tool_call_id":"t1","title":"Write"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(requested).unwrap(),
            Event::ApprovalRequested {
                id: "a1".to_owned(),
                tool_call_id: "t1".to_owned(),
                title: Some("Write".to_owned()),
                target: None,
                folder: None,
                cut: false,
            }
        );
        let failed = r#"{"type":"turn_failed","data":{"message":"gone"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(failed).unwrap(),
            Event::TurnFailed {
                message: "gone".to_owned(),
                cause: None,
                provider: None,
            }
        );
    }

    #[test]
    fn a_tool_call_with_its_details_has_a_stable_stored_shape() {
        assert_eq!(
            serde_json::to_string(&Event::ToolCallUpdated {
                id: "t1".to_owned(),
                title: None,
                status: Some(ToolStatus::Failed),
                kind: Some(ToolKind::Execute),
                input: Some("cargo test".to_owned()),
                output: Some("error".to_owned()),
            })
            .unwrap(),
            r#"{"type":"tool_call_updated","data":{"id":"t1","title":null,"status":"failed","kind":"execute","input":"cargo test","output":"error"}}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::TurnFailed {
                message: "sign in".to_owned(),
                cause: Some(FailureCause::SignInRequired),
                provider: Some("claude".to_owned()),
            })
            .unwrap(),
            r#"{"type":"turn_failed","data":{"message":"sign in","cause":"sign_in_required","provider":"claude"}}"#
        );
    }

    #[test]
    fn a_kind_or_a_cause_this_version_does_not_know_reads_as_other() {
        assert_eq!(
            serde_json::from_str::<ToolKind>(r#""switch_mode""#).unwrap(),
            ToolKind::Other
        );
        assert_eq!(
            serde_json::from_str::<FailureCause>(r#""account_closed""#).unwrap(),
            FailureCause::Other
        );
    }

    #[test]
    fn a_text_is_cut_at_a_character_boundary() {
        let mut short = "caw".to_owned();
        assert!(!cut_text(&mut short, 3));
        assert_eq!(short, "caw");
        let mut accented = "ééé".to_owned();
        assert!(cut_text(&mut accented, 3));
        assert_eq!(accented, "é");
        let mut long = "x".repeat(TOOL_OUTPUT_BYTES + 10);
        assert!(cut_text(&mut long, TOOL_OUTPUT_BYTES));
        assert_eq!(long.len(), TOOL_OUTPUT_BYTES);
    }
}
