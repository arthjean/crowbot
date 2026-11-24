mod bot;
mod endpoint;
mod event;
mod folders;
mod message;
mod preferences;
mod provider;
mod rules;
#[cfg(any(test, feature = "scratch"))]
pub mod scratch;
mod search;
#[cfg(feature = "transport")]
pub mod transport;
mod wire;

pub use bot::{
    AVATAR_COLORS, Avatar, BOT_INSTRUCTIONS_LENGTH, BOT_MODEL_LENGTH, BOT_NAME_LENGTH,
    BOT_TITLE_LENGTH, Bot, BotDraft, BotPatch, BotState, BotSummary, MascotShape, PermissionLevel,
    TOOL_CALL_LIMIT_MAX, mascot_at,
};
pub use endpoint::{Endpoint, SOCKET_PATH_LIMIT};
pub use event::{
    APPROVAL_TARGET_BYTES, ApprovalClosure, ApprovalDecision, ApprovalReason, Cost, Event,
    FailureCause, Interruption, TOOL_INPUT_BYTES, TOOL_OUTPUT_BYTES, ToolEffect, ToolKind,
    ToolStatus, TurnEnd, TurnOrigin, TurnUsage, UserMessage, cut_text,
};
pub use folders::Folders;
pub use message::{
    ClientMessage, ConversationSnapshot, ConversationSummary, MESSAGE_LENGTH, PROTOCOL_VERSION,
    QueueState, QueuedMessage, ServerMessage, TimedEvent,
};
pub use preferences::{
    Appearance, Preferences, PreferencesPatch, SIDEBAR_WIDTH_LIMIT, SendShortcut,
};
pub use provider::{
    DaemonInfo, ModelChoice, ModelsState, ProviderModels, ProviderState, ProviderStatus,
    RuntimeInstall, SignInMethod,
};
pub use rules::{
    ApprovalRule, RULE_ARGUMENT_BYTES, RULE_ARGUMENTS, RULE_PROGRAM_BYTES, RuleDraft, RuleKind,
};
pub use search::{
    ConversationHit, MessageAuthor, MessageHit, SEARCH_MESSAGE_TERM_LENGTH, SEARCH_QUERY_LENGTH,
    SearchResults,
};
pub use wire::{WireError, decode, encode};

/// The exit status of a daemon that did not start because another daemon
/// holds its database. A client that started it waits for that daemon.
pub const EXIT_DATABASE_HELD: u8 = 3;

/// The exit status of a daemon that did not start because another program
/// holds its endpoint. A client that started it attaches to that program.
pub const EXIT_ENDPOINT_HELD: u8 = 4;

/// The exit status of a daemon that did not start because the path of its
/// socket is longer than `SOCKET_PATH_LIMIT`. Nothing was created.
pub const EXIT_ENDPOINT_TOO_LONG: u8 = 5;
