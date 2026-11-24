use serde::{Deserialize, Serialize};

use crate::memory::{MemoryAuthor, MemoryChange};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    UserText(UserMessage),
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
        /// What the tool may change, when it is one of the daemon's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect: Option<ToolEffect>,
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
    /// The agent called a daemon tool that this session may not call, or
    /// that does not exist. The tool did not run.
    ToolRefused {
        tool: String,
        message: String,
    },
    /// The bot's memory changed: what each operation of one call or one
    /// client message did, applied together.
    MemoryChanged {
        author: MemoryAuthor,
        changes: Vec<MemoryChange>,
    },
}

impl Event {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::UserText(UserMessage::new(text))
    }

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
    /// The agent asked to call a tool of the daemon, which the daemon gates
    /// itself where it runs the tool.
    DaemonTool,
    /// A built-in refusal: the command would destroy the computer, the
    /// user's home folder, or the daemon itself, whatever the level.
    Floor,
    /// A rule a person gave the bot, refuse or allow.
    Rule { id: i64 },
    /// The answer could not be written to the database, so the daemon
    /// declined the request rather than let run what the history would miss.
    NotRecorded,
}

/// What a daemon tool may change: nothing, state of the bot that the daemon
/// versions and can put back (memory, skills), or anything else. Only an
/// `External` tool asks a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    Read,
    BotState,
    External,
}

/// Who started a turn: a person, or the daemon on its own for one of its
/// reasons. Every turn the daemon starts needs the switch of its origin
/// (`AGENTS.md`, "A turn the daemon starts needs the user's switch").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    /// A message a person sent from a client.
    #[default]
    User,
    Routine,
    Review,
    Goal,
    Delegation,
    /// The same message, sent again after a turn that failed before it said
    /// anything.
    Retry,
    Budget,
}

/// The message that starts a turn, with the note the daemon put before it:
/// what changed in what the session was given when it opened. A message a
/// person sent with no note is stored as its text alone, the shape every
/// earlier daemon wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserMessage {
    pub text: String,
    pub note: Option<String>,
    pub origin: TurnOrigin,
}

impl UserMessage {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            note: None,
            origin: TurnOrigin::User,
        }
    }

    #[must_use]
    pub fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note.filter(|note| !note.is_empty());
        self
    }

    #[must_use]
    pub fn with_origin(mut self, origin: TurnOrigin) -> Self {
        self.origin = origin;
        self
    }
}

impl std::ops::Deref for UserMessage {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

fn is_user(origin: &TurnOrigin) -> bool {
    *origin == TurnOrigin::User
}

impl Serialize for UserMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Noted<'a> {
            text: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            note: Option<&'a str>,
            #[serde(skip_serializing_if = "is_user")]
            origin: TurnOrigin,
        }
        if self.note.is_none() && is_user(&self.origin) {
            return serializer.serialize_str(&self.text);
        }
        Noted {
            text: &self.text,
            note: self.note.as_deref(),
            origin: self.origin,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UserMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Text(String),
            Noted {
                text: String,
                #[serde(default)]
                note: Option<String>,
                #[serde(default)]
                origin: TurnOrigin,
            },
        }
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Text(text) => Self::new(text),
            Stored::Noted { text, note, origin } => {
                Self::new(text).with_note(note).with_origin(origin)
            }
        })
    }
}

/// What an agent reported a turn consumed. A field the agent did not report
/// is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsage {
    /// The provider that ran the turn.
    pub provider: String,
    /// The tokens in the context at the end of the turn, out of `context_size`.
    #[serde(default)]
    pub context_used: Option<u64>,
    #[serde(default)]
    pub context_size: Option<u64>,
    /// What the provider session has cost so far, at the end of the turn.
    #[serde(default)]
    pub session_cost: Option<Cost>,
    /// What the session cost grew by during the turn.
    #[serde(default)]
    pub turn_cost: Option<Cost>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

/// An amount of money, or of another unit a provider counts in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    pub amount: f64,
    /// An ISO 4217 code such as `USD`, or another unit such as `credits`.
    pub currency: String,
}

/// Two costs are equal when they hold the same bits: a cost is a figure the
/// daemon stores and shows, never one it computes a decision from.
impl PartialEq for Cost {
    fn eq(&self, other: &Self) -> bool {
        self.amount.to_bits() == other.amount.to_bits() && self.currency == other.currency
    }
}

impl Eq for Cost {}

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
    /// The bot repeated the same tool calls with the same results.
    NoProgress,
    /// A turn the daemon started received nothing from its agent for too long.
    Inactive,
    /// The turn asked for more tool calls than the bot's limit per turn.
    BudgetExhausted,
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
    fn an_approval_reason_of_a_rule_carries_the_rule_and_the_others_keep_their_names() {
        let wire = |reason: ApprovalReason| serde_json::to_string(&reason).unwrap();
        assert_eq!(wire(ApprovalReason::Rule { id: 7 }), r#"{"rule":{"id":7}}"#);
        assert_eq!(wire(ApprovalReason::Floor), r#""floor""#);
        assert_eq!(wire(ApprovalReason::NotRecorded), r#""not_recorded""#);
        assert_eq!(
            wire(ApprovalReason::PermissionLevel),
            r#""permission_level""#
        );
        for reason in [
            ApprovalReason::Rule { id: 7 },
            ApprovalReason::Floor,
            ApprovalReason::NotRecorded,
            ApprovalReason::DaemonTool,
        ] {
            assert_eq!(
                serde_json::from_str::<ApprovalReason>(&wire(reason)).unwrap(),
                reason
            );
        }
    }

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
                effect: None,
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
    fn a_message_with_no_note_keeps_the_stored_shape_and_a_note_is_a_field() {
        let plain = r#"{"type":"user_text","data":"hello"}"#;
        assert_eq!(
            serde_json::from_str::<Event>(plain).unwrap(),
            Event::user_text("hello")
        );
        assert_eq!(
            serde_json::to_string(&Event::user_text("hello")).unwrap(),
            plain
        );
        let noted = Event::UserText(UserMessage::new("hi").with_note(Some("changed".to_owned())));
        let stored = serde_json::to_string(&noted).unwrap();
        assert_eq!(
            stored,
            r#"{"type":"user_text","data":{"text":"hi","note":"changed"}}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&stored).unwrap(), noted);
        let no_field = r#"{"type":"user_text","data":{"text":"hi"}}"#;
        assert_eq!(
            serde_json::from_str::<Event>(no_field).unwrap(),
            Event::user_text("hi")
        );
    }

    #[test]
    fn a_message_keeps_its_origin_and_one_stored_before_origins_is_a_person_s() {
        let retried = Event::UserText(UserMessage::new("go").with_origin(TurnOrigin::Retry));
        let stored = serde_json::to_string(&retried).unwrap();
        assert_eq!(
            stored,
            r#"{"type":"user_text","data":{"text":"go","origin":"retry"}}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&stored).unwrap(), retried);
        let noted = r#"{"type":"user_text","data":{"text":"go","note":"changed"}}"#;
        let Event::UserText(message) = serde_json::from_str::<Event>(noted).unwrap() else {
            panic!("not a message");
        };
        assert_eq!(message.origin, TurnOrigin::User);
        let Event::UserText(plain) =
            serde_json::from_str::<Event>(r#"{"type":"user_text","data":"go"}"#).unwrap()
        else {
            panic!("not a message");
        };
        assert_eq!(plain.origin, TurnOrigin::User);
        for (origin, name) in [
            (TurnOrigin::User, "user"),
            (TurnOrigin::Routine, "routine"),
            (TurnOrigin::Review, "review"),
            (TurnOrigin::Goal, "goal"),
            (TurnOrigin::Delegation, "delegation"),
            (TurnOrigin::Retry, "retry"),
            (TurnOrigin::Budget, "budget"),
        ] {
            assert_eq!(
                serde_json::to_string(&origin).unwrap(),
                format!("\"{name}\"")
            );
        }
    }

    #[test]
    fn the_causes_of_a_guarded_stop_have_a_stable_stored_shape() {
        for (cause, name) in [
            (Interruption::NoProgress, "no_progress"),
            (Interruption::Inactive, "inactive"),
            (Interruption::BudgetExhausted, "budget_exhausted"),
        ] {
            assert_eq!(
                serde_json::to_string(&Event::TurnInterrupted { cause }).unwrap(),
                format!(r#"{{"type":"turn_interrupted","data":{{"cause":"{name}"}}}}"#)
            );
        }
    }

    #[test]
    fn a_usage_that_reported_nothing_more_reads_with_empty_fields() {
        let usage: TurnUsage = serde_json::from_str(r#"{"provider":"grok"}"#).unwrap();
        assert_eq!(usage.context_used, None);
        assert_eq!(usage.turn_cost, None);
        let cost = Cost {
            amount: 0.25,
            currency: "USD".to_owned(),
        };
        assert_eq!(cost.clone(), cost);
        assert_ne!(
            cost,
            Cost {
                amount: 0.25,
                currency: "credits".to_owned()
            }
        );
    }

    #[test]
    fn a_daemon_tool_call_and_a_refusal_have_a_stable_stored_shape() {
        let started = Event::ToolCallStarted {
            id: "t1".to_owned(),
            title: "memory".to_owned(),
            status: None,
            kind: None,
            input: None,
            effect: Some(ToolEffect::BotState),
        };
        assert_eq!(
            serde_json::to_string(&started).unwrap(),
            r#"{"type":"tool_call_started","data":{"id":"t1","title":"memory","status":null,"kind":null,"input":null,"effect":"bot_state"}}"#
        );
        let refused = Event::ToolRefused {
            tool: "review_only".to_owned(),
            message: "no".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&refused).unwrap(),
            r#"{"type":"tool_refused","data":{"tool":"review_only","message":"no"}}"#
        );
        assert_eq!(
            serde_json::to_string(&ApprovalReason::DaemonTool).unwrap(),
            r#""daemon_tool""#
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
