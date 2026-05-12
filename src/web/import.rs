//! `POST /api/collection/import` — paste-text collection ingest.
//!
//! Accepts MTG Arena's deck-export format (the most common shape any third-
//! party exporter produces today), e.g.:
//!
//! ```text
//! 4 Lightning Bolt (M21) 162
//! 1 Plains (NEO) 287
//! ```
//!
//! Also tolerates `Deck` / `Sideboard` headers (silently ignored), blank lines,
//! and `// comments`. As a fallback for power users we accept comma-separated
//! `arena_id,quantity` rows when no parens-set-num shape is detected on a line.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cards::CardDb;
use crate::state::handlers::inventory_changes;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ImportRequest {
    pub text: String,
    /// If true, replace the entire collection (zeroing first); if false (default), add to it.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Serialize)]
pub struct ImportSummary {
    pub matched_lines: usize,
    pub unmatched_lines: usize,
    pub unique_cards: usize,
    pub total_cards: u32,
    pub unmatched_samples: Vec<String>,
}

// "4 Lightning Bolt (M21) 162" or "4x Lightning Bolt (M21) 162"
static ARENA_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(\d+)x?\s+(.+?)\s+\(([A-Z0-9]{2,5})\)\s+([A-Z0-9★†_-]+)\s*$").expect("arena re")
});
// "12345,4" or "12345 4" — a fallback for direct grpId,qty rows.
static ID_QTY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(\d{3,7})[\s,]+(\d+)\s*$").expect("id-qty re"));

/// One parsed line from a pasted decklist: `(arena_id, quantity, sideboard?)`.
pub struct ParsedDecklist {
    pub entries: Vec<(u32, u32, bool)>,
    pub matched_lines: usize,
    pub unmatched_lines: usize,
    pub unmatched_samples: Vec<String>,
}

/// Parse Arena-format decklist text (or `arena_id,qty` rows) against `cards`.
/// Tracks which side of a `Deck` / `Sideboard` header each line falls under so
/// the same parser can serve both /collection/import (sideboard ignored) and
/// /decks/analyze (sideboard preserved).
pub fn parse_arena_decklist(text: &str, cards: &CardDb) -> ParsedDecklist {
    let mut matched_lines = 0usize;
    let mut unmatched_lines = 0usize;
    let mut entries: Vec<(u32, u32, bool)> = Vec::new();
    let mut unmatched_samples: Vec<String> = Vec::new();
    let mut in_sideboard = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        match line.to_ascii_lowercase().as_str() {
            "deck" | "commander" | "companion" => { in_sideboard = false; continue; }
            "sideboard" | "maybeboard" => { in_sideboard = true; continue; }
            _ => {}
        }

        if let Some(caps) = ARENA_RE.captures(line) {
            let qty: u32 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
            let set = caps.get(3).unwrap().as_str();
            let num = caps.get(4).unwrap().as_str();
            if let Some(grp) = cards.lookup_set_num(set, num) {
                entries.push((grp, qty, in_sideboard));
                matched_lines += 1;
                continue;
            }
        }
        if let Some(caps) = ID_QTY_RE.captures(line) {
            let id: u32 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
            let qty: u32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
            if id > 0 && qty > 0 {
                entries.push((id, qty, in_sideboard));
                matched_lines += 1;
                continue;
            }
        }

        unmatched_lines += 1;
        if unmatched_samples.len() < 10 {
            unmatched_samples.push(line.to_string());
        }
    }

    ParsedDecklist { entries, matched_lines, unmatched_lines, unmatched_samples }
}

pub async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> impl IntoResponse {
    let parsed = parse_arena_decklist(&req.text, &state.cards);
    let mut additions: std::collections::HashMap<u32, u32> = Default::default();
    for (id, qty, _sideboard) in &parsed.entries {
        *additions.entry(*id).or_insert(0) += qty;
    }

    if req.replace {
        if let Err(e) = sqlx::query("DELETE FROM collection").execute(&state.pool).await {
            tracing::error!(error = %e, "failed to clear collection during replace import");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    }

    for (card_id, qty) in &additions {
        if let Err(e) = inventory_changes::record_delta(&state.pool, *card_id, *qty as i64, "import", None).await {
            tracing::warn!(error = %e, card_id, "failed to record import delta");
        }
    }

    let total: u32 = additions.values().sum();
    let summary = ImportSummary {
        matched_lines: parsed.matched_lines,
        unmatched_lines: parsed.unmatched_lines,
        unique_cards: additions.len(),
        total_cards: total,
        unmatched_samples: parsed.unmatched_samples,
    };
    (StatusCode::OK, Json(summary)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_qty_fallback_with_sideboard_header() {
        let cards = CardDb::empty();
        // Falls back to id,qty rows so the test doesn't need real Scryfall data.
        let text = "Deck\n70000 4\n70001 2\n\nSideboard\n70002 3\n";
        let parsed = parse_arena_decklist(text, &cards);
        assert_eq!(parsed.matched_lines, 3);
        assert_eq!(parsed.unmatched_lines, 0);
        assert_eq!(parsed.entries, vec![(70000, 4, false), (70001, 2, false), (70002, 3, true)]);
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let cards = CardDb::empty();
        let text = "// header\n\n70000 1\n# comment\n";
        let parsed = parse_arena_decklist(text, &cards);
        assert_eq!(parsed.matched_lines, 1);
        assert_eq!(parsed.entries, vec![(70000, 1, false)]);
    }

    #[test]
    fn commander_and_companion_reset_to_mainboard() {
        let cards = CardDb::empty();
        let text = "Sideboard\n70000 1\nCommander\n70001 1\n";
        let parsed = parse_arena_decklist(text, &cards);
        assert_eq!(parsed.entries, vec![(70000, 1, true), (70001, 1, false)]);
    }
}
