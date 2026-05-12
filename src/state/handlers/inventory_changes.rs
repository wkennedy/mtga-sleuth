//! Apply card-grant deltas detected in MTGA's `Changes` arrays and booster /
//! reward events. Updates both the running `collection` total and the
//! `collection_events` audit log.
//!
//! MTGA emits per-context delta records inside `InventoryInfo.Changes`:
//!
//! ```json
//! { "context": "Quest.Completed",
//!   "delta": {"gemsDelta": 50, "cardsAdded": [12345, 67890], "boosterAdded": [...]} }
//! ```
//!
//! Booster open events emit `cardsOpened` / `cardGrants` arrays at top level.
//! We accept any shape that has a numeric array of card IDs.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::parser::ParsedEvent;
use crate::state::AppState;

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let mut additions: HashMap<u32, u32> = HashMap::new();
    walk(&event.payload, &mut additions);

    if additions.is_empty() {
        return Ok(());
    }

    let total: u32 = additions.values().sum();
    tracing::info!(unique = additions.len(), total, source = %event.kind, "collection delta detected");

    for (card_id, qty) in additions {
        record_delta(&state.pool, card_id, qty as i64, "inventory_change", Some(&event.kind)).await?;
    }
    Ok(())
}

/// Walk an arbitrary JSON tree summing card grants into `out`. Recognizes
/// flat arrays of card IDs (`cardsAdded`, `cardsOpened`, `cardGrants`,
/// `boosterContents`) and `Changes`/`changes` arrays whose elements carry a
/// `delta.cardsAdded`.
fn walk(v: &Value, out: &mut HashMap<u32, u32>) {
    match v {
        Value::Object(map) => {
            // Direct grants (booster opens, reward grants).
            for key in ["cardsAdded", "cardsOpened", "cardGrants", "boosterContents", "Cards", "cards"] {
                if let Some(arr) = map.get(key).and_then(|x| x.as_array()) {
                    for el in arr {
                        if let Some(id) = el.as_u64().and_then(|n| u32::try_from(n).ok()) {
                            *out.entry(id).or_insert(0) += 1;
                        } else if let Some(card_id) = el.get("cardId").or_else(|| el.get("grpId")).and_then(|n| n.as_u64()) {
                            let qty = el.get("quantity").and_then(|x| x.as_u64()).unwrap_or(1);
                            *out.entry(card_id as u32).or_insert(0) += qty as u32;
                        }
                    }
                }
            }
            // Recurse into nested values so buried `cardsAdded` arrays inside
            // `Changes`/`delta`/etc. wrappers get picked up. We don't special-
            // case those wrappers — the recursion already covers them, and
            // double-handling caused a counting bug.
            for sub in map.values() {
                if sub.is_object() || sub.is_array() {
                    walk(sub, out);
                }
            }
        }
        Value::Array(arr) => {
            for el in arr {
                walk(el, out);
            }
        }
        _ => {}
    }
}

/// Apply one card delta: bump the running `collection` total and append a row
/// to `collection_events`.
pub async fn record_delta(
    pool: &SqlitePool,
    card_id: u32,
    delta: i64,
    source: &str,
    context: Option<&str>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO collection (card_id, quantity) VALUES (?, ?)
         ON CONFLICT(card_id) DO UPDATE SET quantity = MAX(quantity + excluded.quantity, 0)",
    )
    .bind(card_id as i64)
    .bind(delta)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO collection_events (card_id, delta, source, context) VALUES (?, ?, ?, ?)")
        .bind(card_id as i64)
        .bind(delta)
        .bind(source)
        .bind(context)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_cards_added_in_changes() {
        let mut out = HashMap::new();
        walk(
            &json!({
                "Changes": [
                    {"context": "Quest.Completed", "delta": {"cardsAdded": [12345, 67890, 12345]}},
                    {"context": "Booster.Opened", "delta": {"cardsAdded": [99999]}},
                ]
            }),
            &mut out,
        );
        assert_eq!(out.get(&12345), Some(&2));
        assert_eq!(out.get(&67890), Some(&1));
        assert_eq!(out.get(&99999), Some(&1));
    }

    #[test]
    fn finds_booster_contents_with_quantity() {
        let mut out = HashMap::new();
        walk(
            &json!({"boosterContents": [{"cardId": 100, "quantity": 4}, {"grpId": 200, "quantity": 1}]}),
            &mut out,
        );
        assert_eq!(out.get(&100), Some(&4));
        assert_eq!(out.get(&200), Some(&1));
    }

    #[test]
    fn ignores_payloads_without_card_grants() {
        let mut out = HashMap::new();
        walk(&json!({"Gold": 850, "Gems": 20}), &mut out);
        assert!(out.is_empty());
    }
}
