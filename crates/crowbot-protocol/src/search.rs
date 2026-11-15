use serde::{Deserialize, Serialize};

pub const SEARCH_QUERY_LENGTH: usize = 200;
pub const SEARCH_MESSAGE_TERM_LENGTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationHit {
    pub conversation_id: i64,
    pub bot_id: i64,
    pub title: String,
    pub last_activity_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    User,
    Bot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageHit {
    pub conversation_id: i64,
    pub bot_id: i64,
    pub event_id: i64,
    pub author: MessageAuthor,
    pub snippet: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub request: u64,
    pub query: String,
    #[serde(default)]
    pub bots: Vec<i64>,
    #[serde(default)]
    pub conversations: Vec<ConversationHit>,
    #[serde(default)]
    pub messages: Vec<MessageHit>,
    #[serde(default)]
    pub messages_searched: bool,
}
