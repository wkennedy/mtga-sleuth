//! Decode MTGA deck-list payloads.
//!
//! Modern (2026) MTGA emits two main shapes:
//!
//! 1. `DeckGetAllPreconDecksV3` response — preconstructed decks:
//!    ```json
//!    { "CacheVersion": 555, "PreconDecks": [
//!        { "Summary": { "DeckId": "uuid", "Name": "...", "Attributes": [{"name":"Format","value":"Brawl"}] },
//!          "Deck": { "MainDeck": [{"cardId": 69227, "quantity": 1}, ...],
//!                    "Sideboard": [...] } },
//!        ...
//!    ] }
//!    ```
//!
//! 2. `DeckUpsertDeckV3` response — single user deck (same nested shape).
//!
//! 3. Legacy: flat `{id, name, mainDeck, sideboard}` (older tracker docs / old
//!    log format). Still supported for forward-compat.
//!
//! Card lists may use `cardId`/`quantity` (modern) or `id`/`quantity` (legacy)
//! or even a flat `[id, qty, id, qty, ...]` array (very old format).

use std::collections::HashMap;

use anyhow::Result;
use serde_json::Value;

use crate::parser::ParsedEvent;
use crate::state::{AppState, DeckSummary};

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let decks = extract_deck_array(&event.payload);
    if decks.is_empty() {
        tracing::trace!(kind = %event.kind, "no decks found in payload");
        return Ok(());
    }
    tracing::info!(kind = %event.kind, count = decks.len(), "ingesting decks");
    for deck in decks {
        if let Err(e) = persist_deck(state, &deck, &event.kind).await {
            tracing::warn!(error = %e, "failed to persist deck");
        }
    }
    Ok(())
}

/// Classify a deck as the player's own or a precon/reference deck.
///
/// The precon catalog event is all precons by definition. StartHook's `Decks`
/// map mixes both: the player's decks have human-typed names, while precons
/// carry unlocalized `?=?Loc/Decks/Precon/...` name keys (verified empirically
/// 2026-08-13).
fn classify_origin(event_kind: &str, name: &str) -> &'static str {
    if event_kind.contains("Precon") || name.starts_with("?=?Loc/Decks/Precon") {
        "precon"
    } else {
        "personal"
    }
}

/// Find the array of deck-shaped objects inside any of the known wrappers.
fn extract_deck_array(payload: &Value) -> Vec<Value> {
    if let Some(arr) = payload.as_array() {
        return arr.clone();
    }
    if let Some(obj) = payload.as_object() {
        for key in ["PreconDecks", "Decks", "decks", "decklists", "PlayerDecks", "playerDecks"] {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                return arr.clone();
            }
        }
        // Single-deck shapes:
        if obj.contains_key("Summary") && obj.contains_key("Deck") {
            return vec![payload.clone()];
        }
        if obj.contains_key("id") && obj.contains_key("name") {
            return vec![payload.clone()];
        }
        if obj.contains_key("DeckId") && (obj.contains_key("Name") || obj.contains_key("MainDeck")) {
            return vec![payload.clone()];
        }
    }
    Vec::new()
}

async fn persist_deck(state: &AppState, deck: &Value, event_kind: &str) -> Result<()> {
    // Try several locations for each field — modern (Summary.DeckId), legacy (id), and flat.
    let summary = deck.get("Summary");
    let deck_body = deck.get("Deck").unwrap_or(deck);

    let Some(deck_id) = pluck_str(deck, summary, &["DeckId", "deckId", "id"]) else {
        tracing::trace!("deck object missing id; skipping");
        return Ok(());
    };
    let name = pluck_str(deck, summary, &["Name", "name"]).unwrap_or_else(|| "Unnamed".into());
    let format = extract_format(deck, summary);
    let origin = classify_origin(event_kind, &name);
    let tile_card_id = pluck_u32(deck, summary, &["DeckTileId", "deckTileId", "DeckArtId", "deckArtId"]);

    let main = parse_card_list(deck_body.get("MainDeck").or_else(|| deck_body.get("mainDeck")));
    let side = parse_card_list(deck_body.get("Sideboard").or_else(|| deck_body.get("sideboard")));

    let mut tx = state.pool.begin().await?;
    // `origin` upgrades to personal but never downgrades: starter precons the
    // player has claimed appear plain-named in StartHook's deck list (=
    // personal) AND under the same deck_id in the precon catalog event, and
    // the catalog must not win regardless of event order. `tile_card_id`
    // keeps its old value when an event doesn't carry one.
    sqlx::query(
        "INSERT INTO decks (deck_id, name, format, origin, tile_card_id, last_updated) VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(deck_id) DO UPDATE SET name = excluded.name, format = excluded.format,
             origin = CASE WHEN decks.origin = 'personal' THEN 'personal' ELSE excluded.origin END,
             tile_card_id = COALESCE(excluded.tile_card_id, decks.tile_card_id),
             last_updated = CURRENT_TIMESTAMP",
    )
    .bind(&deck_id)
    .bind(&name)
    .bind(&format)
    .bind(origin)
    .bind(tile_card_id.map(|v| v as i64))
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM deck_cards WHERE deck_id = ?")
        .bind(&deck_id)
        .execute(&mut *tx)
        .await?;

    for (card_id, qty) in main.iter() {
        sqlx::query("INSERT INTO deck_cards (deck_id, card_id, quantity, sideboard) VALUES (?, ?, ?, 0)")
            .bind(&deck_id)
            .bind(*card_id as i64)
            .bind(*qty as i64)
            .execute(&mut *tx)
            .await?;
    }
    for (card_id, qty) in side.iter() {
        sqlx::query("INSERT INTO deck_cards (deck_id, card_id, quantity, sideboard) VALUES (?, ?, ?, 1)")
            .bind(&deck_id)
            .bind(*card_id as i64)
            .bind(*qty as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let summary_row = DeckSummary {
        deck_id: deck_id.clone(),
        name,
        format,
        mainboard: main,
        sideboard: side,
    };
    state.deck_index.write().await.insert(deck_id, summary_row);
    Ok(())
}

fn pluck_str(root: &Value, summary: Option<&Value>, keys: &[&str]) -> Option<String> {
    for src in [summary, Some(root)].iter().flatten() {
        for k in keys {
            if let Some(s) = src.get(*k).and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn pluck_u32(root: &Value, summary: Option<&Value>, keys: &[&str]) -> Option<u32> {
    for src in [summary, Some(root)].iter().flatten() {
        for k in keys {
            if let Some(n) = src.get(*k).and_then(any_to_u32) {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn extract_format(root: &Value, summary: Option<&Value>) -> Option<String> {
    // Direct key first.
    if let Some(f) = pluck_str(root, summary, &["Format", "format"]) {
        return Some(f);
    }
    // Modern shape: Attributes: [{name: "Format", value: "Brawl"}, ...]
    let attrs = summary
        .and_then(|s| s.get("Attributes"))
        .or_else(|| root.get("Attributes"))
        .and_then(|v| v.as_array())?;
    for a in attrs {
        if a.get("name").and_then(|v| v.as_str()) == Some("Format") {
            if let Some(v) = a.get("value").and_then(|v| v.as_str()) {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Accept `[{cardId|id, quantity}]`, `[id, qty, id, qty, ...]`, or `[id, id, id]`.
fn parse_card_list(v: Option<&Value>) -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    let Some(v) = v else { return out };
    let Some(arr) = v.as_array() else { return out };

    if arr.iter().all(|el| el.is_object()) {
        for el in arr {
            let id = el.get("cardId")
                .or_else(|| el.get("CardId"))
                .or_else(|| el.get("id"))
                .or_else(|| el.get("Id"))
                .and_then(any_to_u32);
            let qty = el.get("quantity")
                .or_else(|| el.get("Quantity"))
                .and_then(any_to_u32)
                .unwrap_or(1);
            if let Some(id) = id {
                *out.entry(id).or_insert(0) += qty;
            }
        }
    } else if arr.iter().all(|el| el.is_number() || el.is_string()) {
        // Flat alternating list (very old format) vs flat id-only list.
        // Heuristic: if every other element is a small number (qty ≤ 4), treat as pairs.
        let looks_like_pairs = arr.len() >= 2
            && arr.iter().enumerate().filter(|(i, _)| i % 2 == 1).all(|(_, v)| v.as_u64().is_some_and(|n| n <= 30));
        if looks_like_pairs {
            let mut i = 0;
            while i + 1 < arr.len() {
                if let (Some(id), Some(qty)) = (any_to_u32(&arr[i]), any_to_u32(&arr[i + 1])) {
                    *out.entry(id).or_insert(0) += qty;
                }
                i += 2;
            }
        } else {
            for el in arr {
                if let Some(id) = any_to_u32(el) {
                    *out.entry(id).or_insert(0) += 1;
                }
            }
        }
    }
    out
}

fn any_to_u32(v: &Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return u32::try_from(n).ok();
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_precon_array() {
        let payload = json!({
            "CacheVersion": 1,
            "PreconDecks": [
                {"Summary": {"DeckId": "a"}, "Deck": {"MainDeck": []}},
                {"Summary": {"DeckId": "b"}, "Deck": {"MainDeck": []}},
            ]
        });
        let decks = extract_deck_array(&payload);
        assert_eq!(decks.len(), 2);
    }

    #[test]
    fn parses_modern_card_list() {
        let v = json!([
            {"cardId": 69227, "quantity": 4},
            {"cardId": 73363, "quantity": 2},
        ]);
        let out = parse_card_list(Some(&v));
        assert_eq!(out.get(&69227), Some(&4));
        assert_eq!(out.get(&73363), Some(&2));
    }

    #[test]
    fn extracts_format_from_attributes() {
        let summary = json!({"DeckId": "x", "Name": "y", "Attributes": [
            {"name": "Format", "value": "Brawl"},
            {"name": "Other", "value": "ignored"}
        ]});
        let fmt = extract_format(&Value::Null, Some(&summary));
        assert_eq!(fmt.as_deref(), Some("Brawl"));
    }

    #[test]
    fn classifies_deck_origin() {
        // Precon catalog event: everything is a precon regardless of name.
        assert_eq!(classify_origin("DeckGetAllPreconDecksV3", "Aerial Domination"), "precon");
        // StartHook mixes both; the loc-key name prefix identifies precons.
        assert_eq!(classify_origin("StartHook.Decks", "?=?Loc/Decks/Precon/Precon_EPP_W_Name"), "precon");
        assert_eq!(classify_origin("StartHook.Decks", "Simic Flash (Imp)"), "personal");
        // Deck edits are always the player's.
        assert_eq!(classify_origin("DeckUpsertDeckV3", "My Brew"), "personal");
    }

    #[test]
    fn flat_id_only_list_treated_as_singletons() {
        // Used for draft pack inspection; each id appears once.
        let v = json!([69227, 73363, 72403]);
        let out = parse_card_list(Some(&v));
        assert_eq!(out.len(), 3);
        assert_eq!(out.get(&69227), Some(&1));
    }
}
