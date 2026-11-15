mod bot;
mod endpoint;
mod event;
mod message;
mod preferences;
mod provider;
mod search;
#[cfg(feature = "transport")]
pub mod transport;
mod wire;

pub use bot::{
    AVATAR_COLORS, Avatar, BOT_INSTRUCTIONS_LENGTH, BOT_MODEL_LENGTH, BOT_NAME_LENGTH,
    BOT_TITLE_LENGTH, Bot, BotDraft, BotPatch, BotState, BotSummary, PermissionLevel,
};
pub use endpoint::Endpoint;
pub use event::{
    APPROVAL_TARGET_BYTES, ApprovalClosure, ApprovalDecision, ApprovalReason, Event, FailureCause,
    Interruption, TOOL_INPUT_BYTES, TOOL_OUTPUT_BYTES, ToolKind, ToolStatus, TurnEnd, cut_text,
};
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
pub use search::{
    ConversationHit, MessageAuthor, MessageHit, SEARCH_MESSAGE_TERM_LENGTH, SEARCH_QUERY_LENGTH,
    SearchResults,
};
pub use wire::{WireError, decode, encode};
