//! Decode collection updates.
//!
//! MTGA emits collection in a few shapes; the most common is
//! `{ "<arena_id>": <count>, ... }` either at payload root or under a `cards`
//! / `playerCards` / `cardPool` key.

use anyhow::Result;
use serde_json::{Map, Value};

use crate::parser::ParsedEvent;
use crate::state::AppState;

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let map = locate_collection(&event.payload);
    if map.is_empty() {
        tracing::trace!(kind = %event.kind, "no collection map in payload");
        return Ok(());
    }
    let mut tx = state.pool.begin().await?;
    for (k, v) in &map {
        let Ok(card_id) = k.parse::<u32>() else { continue };
        let Some(qty) = v.as_u64() else { continue };
        sqlx::query(
            "INSERT INTO collection (card_id, quantity) VALUES (?, ?)
             ON CONFLICT(card_id) DO UPDATE SET quantity = excluded.quantity",
        )
        .bind(card_id as i64)
        .bind(qty as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(count = map.len(), "collection updated");
    Ok(())
}

fn locate_collection(payload: &Value) -> Map<String, Value> {
    if let Some(obj) = payload.as_object() {
        if obj.keys().all(|k| k.parse::<u32>().is_ok()) && !obj.is_empty() {
            return obj.clone();
        }
        for key in ["cards", "playerCards", "cardPool", "collection", "playerCardsV3"] {
            if let Some(inner) = obj.get(key).and_then(|v| v.as_object()) {
                return inner.clone();
            }
        }
    }
    Map::new()
}
