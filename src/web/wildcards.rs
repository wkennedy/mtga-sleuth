//! Collection-aware deck analysis: for a list of `(arena_id, qty, sideboard)`
//! rows, joins against the user's `collection` table and computes per-card
//! `owned`/`missing` counts plus a wildcard-cost total broken down by rarity.
//!
//! Basic lands (`type_line` contains "Basic Land") are treated as fully owned —
//! Arena gives them out for free, so they never contribute to wildcard cost.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::cards::rewrite_image_url;
use crate::state::AppState;
use crate::web::api::{ApiError, DeckCardEntry};

#[derive(Default, Clone, Serialize)]
pub struct WildcardCost {
    pub common: u32,
    pub uncommon: u32,
    pub rare: u32,
    pub mythic: u32,
}

#[derive(Default, Clone, Serialize)]
pub struct DeckCost {
    pub wildcards_needed: WildcardCost,
    pub total_missing: u32,
}

pub struct Analysis {
    pub mainboard: Vec<DeckCardEntry>,
    pub sideboard: Vec<DeckCardEntry>,
    pub cost: WildcardCost,
    pub unique_missing: u32,
    pub total_missing: u32,
}

/// Compute owned/missing per card and aggregate wildcard cost.
///
/// `rows` is `(card_id, quantity, sideboard_flag)`. Mainboard + sideboard
/// share the collection pool: a card in both is counted as one combined
/// requirement against the owned count, then the missing copies are
/// distributed back proportionally so each entry shows a sensible per-row
/// owned figure.
pub async fn analyze(
    state: &AppState,
    pool: &SqlitePool,
    rows: Vec<(i64, i64, i64)>,
) -> Result<Analysis, ApiError> {
    let unique_ids: HashSet<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    let owned = fetch_owned(pool, &unique_ids).await?;

    // Aggregate required across mainboard + sideboard so a 4-of in main and
    // 2-of in side correctly demands 6 copies before anything is "missing".
    let mut required_total: HashMap<i64, u32> = HashMap::new();
    for (id, qty, _) in &rows {
        *required_total.entry(*id).or_insert(0) += *qty as u32;
    }

    let mut cost = WildcardCost::default();
    let mut unique_missing = 0u32;
    let mut total_missing = 0u32;
    let mut missing_per_card: HashMap<i64, u32> = HashMap::new();
    for (id, required) in &required_total {
        let card = state.cards.get(*id as u32);
        if is_basic_land(card.and_then(|c| c.type_line.as_deref())) {
            continue;
        }
        let have = owned.get(id).copied().unwrap_or(0);
        let missing = required.saturating_sub(have);
        if missing > 0 {
            unique_missing += 1;
            total_missing += missing;
            missing_per_card.insert(*id, missing);
            match card.and_then(|c| c.rarity.as_deref()) {
                Some("common") => cost.common += missing,
                Some("uncommon") => cost.uncommon += missing,
                Some("rare") => cost.rare += missing,
                Some("mythic") => cost.mythic += missing,
                _ => {}
            }
        }
    }

    // Distribute the per-card missing total back across mainboard/sideboard
    // rows in row order, so the per-line UI shows where the gaps fall.
    let mut remaining_missing = missing_per_card.clone();
    let (main_rows, side_rows): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, _, s)| *s == 0);
    let mainboard = hydrate(state, main_rows, &owned, &mut remaining_missing);
    let sideboard = hydrate(state, side_rows, &owned, &mut remaining_missing);

    Ok(Analysis { mainboard, sideboard, cost, unique_missing, total_missing })
}

fn hydrate(
    state: &AppState,
    rows: Vec<(i64, i64, i64)>,
    owned: &HashMap<i64, u32>,
    remaining_missing: &mut HashMap<i64, u32>,
) -> Vec<DeckCardEntry> {
    rows.into_iter()
        .map(|(card_id, qty, _)| {
            let card = state.cards.get(card_id as u32);
            let qty_u = qty as u32;
            let have_total = owned.get(&card_id).copied().unwrap_or(0);
            let basic = is_basic_land(card.and_then(|c| c.type_line.as_deref()));
            // Per-row missing: pull from the shared pool, capped at this row's quantity.
            // Basic lands never report missing.
            let row_missing = if basic {
                0
            } else {
                let pool = remaining_missing.entry(card_id).or_insert(0);
                let take = (*pool).min(qty_u);
                *pool -= take;
                take
            };
            // Display owned capped at qty so each row reads as "X of N", not
            // "12 of 4" when the collection is overstuffed.
            let _ = have_total;
            let row_owned = if basic { qty_u } else { qty_u.saturating_sub(row_missing) };
            DeckCardEntry {
                arena_id: card_id as u32,
                quantity: qty_u,
                name: card.map(|c| c.name.clone()).unwrap_or_else(|| format!("Card #{card_id}")),
                mana_cost: card.and_then(|c| c.mana_cost.clone()),
                cmc: card.and_then(|c| c.cmc),
                rarity: card.and_then(|c| c.rarity.clone()),
                type_line: card.and_then(|c| c.type_line.clone()),
                image_small: card.and_then(|c| c.image_small.as_deref().map(rewrite_image_url)),
                owned: row_owned,
                missing: row_missing,
                legalities: card.and_then(|c| c.legalities.clone()),
            }
        })
        .collect()
}

pub(crate) async fn fetch_owned(
    pool: &SqlitePool,
    ids: &HashSet<i64>,
) -> Result<HashMap<i64, u32>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // SQLite has no array binding; build an IN-list with placeholders. Counts
    // are bounded by deck size (~75 unique cards), so the query stays small.
    let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!("SELECT card_id, quantity FROM collection WHERE card_id IN ({placeholders})");
    let mut q = sqlx::query_as::<_, (i64, i64)>(&sql);
    for id in ids {
        q = q.bind(*id);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, qty)| (id, qty.max(0) as u32)).collect())
}

fn is_basic_land(type_line: Option<&str>) -> bool {
    type_line.is_some_and(|t| t.contains("Basic Land"))
}

/// Wildcard cost for every deck in one pass, for the deck-list view. A single
/// grouped query beats running `analyze` once per deck (hundreds of decks once
/// precons are counted). Mirrors `analyze`'s semantics: mainboard + sideboard
/// quantities combine into one requirement per card, basic lands are free.
pub async fn costs_for_all_decks(
    state: &AppState,
    pool: &SqlitePool,
) -> Result<HashMap<String, DeckCost>, ApiError> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT dc.deck_id, dc.card_id, SUM(dc.quantity), COALESCE(MAX(c.quantity), 0)
         FROM deck_cards dc LEFT JOIN collection c ON c.card_id = dc.card_id
         GROUP BY dc.deck_id, dc.card_id",
    )
    .fetch_all(pool)
    .await?;

    let mut out: HashMap<String, DeckCost> = HashMap::new();
    for (deck_id, card_id, required, owned) in rows {
        let entry = out.entry(deck_id).or_default();
        let card = state.cards.get(card_id as u32);
        if is_basic_land(card.and_then(|c| c.type_line.as_deref())) {
            continue;
        }
        let missing = (required - owned).max(0) as u32;
        if missing == 0 {
            continue;
        }
        entry.total_missing += missing;
        match card.and_then(|c| c.rarity.as_deref()) {
            Some("common") => entry.wildcards_needed.common += missing,
            Some("uncommon") => entry.wildcards_needed.uncommon += missing,
            Some("rare") => entry.wildcards_needed.rare += missing,
            Some("mythic") => entry.wildcards_needed.mythic += missing,
            _ => {}
        }
    }
    Ok(out)
}

// ---- POST /api/decks/analyze --------------------------------------------------

#[derive(Deserialize)]
pub struct AnalyzeRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct AnalyzeResponse {
    pub mainboard: Vec<DeckCardEntry>,
    pub sideboard: Vec<DeckCardEntry>,
    pub wildcards_needed: WildcardCost,
    pub unique_missing: u32,
    pub total_missing: u32,
    pub matched_lines: usize,
    pub unmatched_lines: usize,
    pub unmatched_samples: Vec<String>,
}

pub async fn analyze_pasted(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AnalyzeRequest>,
) -> Result<Json<AnalyzeResponse>, ApiError> {
    let parsed = crate::web::import::parse_arena_decklist(&req.text, &state.cards);
    let mut rows: Vec<(i64, i64, i64)> = Vec::new();
    for (arena_id, qty, sideboard) in parsed.entries {
        rows.push((arena_id as i64, qty as i64, if sideboard { 1 } else { 0 }));
    }
    let analysis = analyze(&state, &state.pool, rows).await?;
    Ok(Json(AnalyzeResponse {
        mainboard: analysis.mainboard,
        sideboard: analysis.sideboard,
        wildcards_needed: analysis.cost,
        unique_missing: analysis.unique_missing,
        total_missing: analysis.total_missing,
        matched_lines: parsed.matched_lines,
        unmatched_lines: parsed.unmatched_lines,
        unmatched_samples: parsed.unmatched_samples,
    }))
}
