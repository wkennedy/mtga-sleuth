use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Client → server request (`==>` in the log).
    Request,
    /// Server → client response/event (`<==` in the log).
    Response,
    /// Free-form interesting line (no arrow).
    Note,
}

/// A single decoded event from Player.log.
///
/// `kind` is the MTGA-internal event name (e.g. `"Deck.GetDeckListsV3"`,
/// `"MatchGameRoomStateChangedEvent"`, `"GreToClientEvent"`). `payload` is the
/// raw JSON body. Higher layers do typed deserialization on demand.
#[derive(Debug, Clone)]
pub struct ParsedEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub direction: Direction,
    pub payload: Value,
}
