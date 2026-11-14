mod endpoint;
mod event;
mod message;
#[cfg(feature = "transport")]
pub mod transport;
mod wire;

pub use endpoint::Endpoint;
pub use event::{ApprovalClosure, ApprovalDecision, Event, Interruption, ToolStatus, TurnEnd};
pub use message::{
    ClientMessage, ConversationSnapshot, ConversationSummary, PROTOCOL_VERSION, QueueState,
    QueuedMessage, ServerMessage,
};
pub use wire::{WireError, decode, encode};
