use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub enum WireError {
    Json(serde_json::Error),
    TooLong { limit: usize },
    Io(std::io::Error),
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "not a valid message: {error}"),
            Self::TooLong { limit } => {
                write!(formatter, "a message is longer than {limit} bytes")
            }
            Self::Io(error) => write!(formatter, "connection error: {error}"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<serde_json::Error> for WireError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for WireError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, WireError> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    Ok(line)
}

pub fn decode<T: DeserializeOwned>(line: &[u8]) -> Result<T, WireError> {
    Ok(serde_json::from_slice(line)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMessage, Event, ServerMessage};

    #[test]
    fn a_message_is_one_line_even_when_its_text_has_line_breaks() {
        let message = ClientMessage::SendMessage {
            conversation_id: 7,
            text: "first\nsecond".to_owned(),
        };
        let line = encode(&message).unwrap();
        assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(decode::<ClientMessage>(&line).unwrap(), message);
    }

    #[test]
    fn a_server_event_round_trips() {
        let message = ServerMessage::ConversationEvent {
            conversation_id: 3,
            event: Event::MessageText("caw".to_owned()),
        };
        let line = encode(&message).unwrap();
        assert_eq!(decode::<ServerMessage>(&line).unwrap(), message);
    }

    #[test]
    fn text_that_is_not_a_message_is_an_error() {
        assert!(matches!(
            decode::<ClientMessage>(b"GET / HTTP/1.1"),
            Err(WireError::Json(_))
        ));
    }
}
