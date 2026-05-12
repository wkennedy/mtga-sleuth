//! Decode `MatchGameRoomStateChangedEvent` — fires on match queue, start, end.
//!
//! Payload shape (abridged):
//! { "matchGameRoomStateChangedEvent": {
//!     "gameRoomInfo": {
//!         "gameRoomConfig": {
//!             "matchId": "...",
//!             "reservedPlayers": [{"playerName":"X#1234","userId":"..."}, ...],
//!             "eventId": "Standard_Play"
//!         },
//!         "stateType": "MatchGameRoomStateType_Playing|MatchCompleted",
//!         "finalMatchResult": { "resultList": [...] }
//!     }
//! } }

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;

use crate::parser::ParsedEvent;
use crate::state::{AppState, LiveMatch};

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let inner = event
        .payload
        .get("matchGameRoomStateChangedEvent")
        .and_then(|v| v.get("gameRoomInfo"))
        .unwrap_or(&event.payload);

    let state_type = inner.get("stateType").and_then(|v| v.as_str()).unwrap_or("");
    let cfg = inner.get("gameRoomConfig");
    let match_id = cfg.and_then(|c| c.get("matchId")).and_then(|v| v.as_str()).map(String::from);
    let event_id = cfg.and_then(|c| c.get("eventId")).and_then(|v| v.as_str()).map(String::from);

    let opponent = cfg
        .and_then(|c| c.get("reservedPlayers"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find_map(|p| {
            // Heuristic: opponent has a different userId than the local player.
            // We don't reliably know our own userId from this payload alone, so
            // for MVP we pick the first player whose name doesn't look local.
            // A future iteration should cross-reference with login events.
            p.get("playerName").and_then(|v| v.as_str()).map(String::from)
        }));

    match state_type {
        "MatchGameRoomStateType_Playing" | "MatchGameRoomStateType_MatchInProgress" => {
            tracing::info!(?match_id, ?event_id, "match started");
            if let Some(id) = match_id.as_ref() {
                sqlx::query(
                    "INSERT INTO matches (match_id, started_at, opponent_screen_name, event_name)
                     VALUES (?, CURRENT_TIMESTAMP, ?, ?)
                     ON CONFLICT(match_id) DO UPDATE SET event_name = excluded.event_name",
                )
                .bind(id)
                .bind(opponent.as_deref())
                .bind(event_id.as_deref())
                .execute(&state.pool)
                .await?;
            }
            let mut live = LiveMatch::default();
            live.match_id = match_id;
            live.started_at = Some(Utc::now());
            live.opponent_screen_name = opponent;
            *state.current.write().await = Some(live);
        }
        "MatchGameRoomStateType_MatchCompleted" => {
            let won = decode_won(inner);
            tracing::info!(?match_id, ?won, "match ended");
            if let Some(id) = match_id.as_ref() {
                sqlx::query(
                    "UPDATE matches SET ended_at = CURRENT_TIMESTAMP, won = ? WHERE match_id = ?",
                )
                .bind(won.map(|w| w as i64))
                .bind(id)
                .execute(&state.pool)
                .await?;
            }
            *state.current.write().await = None;
        }
        _ => {
            tracing::trace!(state_type, "ignoring match-room state");
        }
    }
    Ok(())
}

fn decode_won(inner: &Value) -> Option<bool> {
    let result = inner.get("finalMatchResult")?.get("resultList")?.as_array()?;
    let last = result.iter().rev().find(|r| r.get("scope").and_then(|v| v.as_str()) == Some("MatchScope_Match"))?;
    let winning = last.get("winningTeamId")?.as_u64()?;
    // MTGA assigns teamId 1 to the local player by convention. This is a
    // heuristic — improve once we read systemSeatIds from the room config.
    Some(winning == 1)
}
