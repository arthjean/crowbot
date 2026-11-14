mod bot;
mod endpoint;
mod event;
mod message;
mod preferences;
#[cfg(feature = "transport")]
pub mod transport;
mod wire;

pub use bot::{
    Avatar, BOT_NAME_LENGTH, Bot, BotDraft, BotPatch, BotState, BotSummary, PermissionLevel,
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
pub use wire::{WireError, decode, encode};
