use serde::{Deserialize, Serialize};

/// The longest program name a rule can hold, in bytes.
pub const RULE_PROGRAM_BYTES: usize = 200;
/// The most leading arguments a rule can hold.
pub const RULE_ARGUMENTS: usize = 32;
/// The longest argument a rule can hold, in bytes.
pub const RULE_ARGUMENT_BYTES: usize = 1_000;

/// What a rule does to a command it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Declined at once, on any level. A refuse rule wins over an allow rule.
    Refuse,
    /// Allowed once without a person, when the command runs in the bot's folder.
    Allow,
}

/// A rule a person gave one bot: a program and the arguments a command
/// starts with. Only a client message writes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRule {
    pub id: i64,
    pub bot_id: i64,
    pub kind: RuleKind,
    pub program: String,
    pub args: Vec<String>,
    pub created_at_ms: i64,
}

/// What a client gives to create a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDraft {
    pub kind: RuleKind,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}
