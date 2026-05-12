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

pub async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> impl IntoResponse {
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    let mut additions: std::collections::HashMap<u32, u32> = Default::default();
    let mut unmatched_samples: Vec<String> = Vec::new();

    for raw in req.text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        // Section headers like "Deck" / "Sideboard" / "Commander".
        if matches!(line.to_ascii_lowercase().as_str(), "deck" | "sideboard" | "commander" | "companion" | "maybeboard") {
            continue;
        }

        if let Some(caps) = ARENA_RE.captures(line) {
            let qty: u32 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
            let set = caps.get(3).unwrap().as_str();
            let num = caps.get(4).unwrap().as_str();
            if let Some(grp) = state.cards.lookup_set_num(set, num) {
                *additions.entry(grp).or_insert(0) += qty;
                matched += 1;
                continue;
            }
        }
        if let Some(caps) = ID_QTY_RE.captures(line) {
            let id: u32 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
            let qty: u32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
            if id > 0 && qty > 0 {
                *additions.entry(id).or_insert(0) += qty;
                matched += 1;
                continue;
            }
        }

        unmatched += 1;
        if unmatched_samples.len() < 10 {
            unmatched_samples.push(line.to_string());
        }
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
        matched_lines: matched,
        unmatched_lines: unmatched,
        unique_cards: additions.len(),
        total_cards: total,
        unmatched_samples,
    };
    (StatusCode::OK, Json(summary)).into_response()
}
