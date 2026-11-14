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
    ApprovalClosure, ApprovalDecision, ApprovalReason, Event, Interruption, ToolStatus, TurnEnd,
};
pub use message::{
    ClientMessage, ConversationSnapshot, ConversationSummary, PROTOCOL_VERSION, QueueState,
    QueuedMessage, ServerMessage,
};
pub use preferences::{Appearance, Preferences, PreferencesPatch, SIDEBAR_WIDTH_LIMIT};
pub use wire::{WireError, decode, encode};
