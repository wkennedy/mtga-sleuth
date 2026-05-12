//! `StartHook` is MTGA's master "initial state dump" emitted once at game
//! launch. It carries:
//!
//! - `InventoryInfo` — wallet, wildcards, vault progress, boosters
//! - `DeckSummaries` — array of `{DeckId, Name, Attributes, ...}`
//! - `Decks` — map of `<deck_id> → {MainDeck, Sideboard, CommandZone, ...}`
//! - `CardMetadataInfo`, `TokenDefinitions`, `PreferredCosmetics`, etc.
//!
//! We split the payload here and dispatch to the focused handlers.

use anyhow::Result;
use serde_json::{json, Value};

use crate::parser::{Direction, ParsedEvent};
use crate::state::handlers::{decks, wallet};
use crate::state::AppState;

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let payload = &event.payload;

    if let Some(inv) = payload.get("InventoryInfo") {
        if let Err(e) = wallet::handle(state, inv).await {
            tracing::warn!(error = %e, "wallet handler failed");
        }
    }

    if let Some(decks_array) = build_combined_decks(payload) {
        let synthetic = ParsedEvent {
            timestamp: event.timestamp,
            kind: "StartHook.Decks".into(),
            direction: Direction::Note,
            payload: json!({ "PreconDecks": decks_array }),
        };
        if let Err(e) = decks::handle(state, &synthetic).await {
            tracing::warn!(error = %e, "decks handler (StartHook) failed");
        }
    }
    Ok(())
}

/// Combine `DeckSummaries` (list with names/format) with `Decks` (dict keyed by
/// DeckId with card lists) into the same `{Summary, Deck}` shape that the
/// existing decks handler already understands.
fn build_combined_decks(payload: &Value) -> Option<Vec<Value>> {
    let summaries = payload.get("DeckSummaries")?.as_array()?;
    let decks = payload.get("Decks")?.as_object()?;
    let mut out = Vec::with_capacity(summaries.len());
    for s in summaries {
        let id = s.get("DeckId").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() { continue; }
        let body = decks.get(id).cloned().unwrap_or(Value::Null);
        out.push(json!({ "Summary": s, "Deck": body }));
    }
    if out.is_empty() { None } else { Some(out) }
}
