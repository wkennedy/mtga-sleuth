//! `POST /api/decks/{id}/simulate` — Monte Carlo draw simulation over a saved
//! deck's mainboard. Pure CPU work; runs under `spawn_blocking` and persists
//! nothing.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use rand::rngs::StdRng;
use rand::SeedableRng;
use serde::Deserialize;

use crate::sim::{self, SimParams, SimResult};
use crate::state::AppState;

use super::api::ApiError;

const DEFAULT_ITERATIONS: u32 = 10_000;
const MAX_ITERATIONS: u32 = 100_000;
const TURNS: u8 = 10;

#[derive(Deserialize, Default)]
pub struct SimulateRequest {
    pub iterations: Option<u32>,
    pub on_play: Option<bool>,
    pub bo1_smoothing: Option<bool>,
    /// Optional RNG seed for reproducible runs; the effective seed is echoed
    /// back in `params.seed` either way.
    pub seed: Option<u64>,
}

pub async fn simulate_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimResult>, ApiError> {
    let exists: Option<(String,)> = sqlx::query_as("SELECT deck_id FROM decks WHERE deck_id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    exists.ok_or(ApiError::NotFound)?;

    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT card_id, quantity FROM deck_cards WHERE deck_id = ? AND sideboard = 0",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    let entries: Vec<(u32, u32)> =
        rows.iter().map(|&(id, qty)| (id as u32, qty.max(0) as u32)).collect();
    let total: u32 = entries.iter().map(|&(_, q)| q).sum();
    if total < 7 {
        return Err(ApiError::BadRequest(format!(
            "deck has only {total} mainboard cards; too small to simulate"
        )));
    }

    let seed = req.seed.unwrap_or_else(rand::random);
    let params = SimParams {
        iterations: req.iterations.unwrap_or(DEFAULT_ITERATIONS).clamp(100, MAX_ITERATIONS),
        on_play: req.on_play.unwrap_or(true),
        bo1_smoothing: req.bo1_smoothing.unwrap_or(false),
        seed: Some(seed),
        turns: TURNS,
    };

    let cards = state.cards.clone();
    let result = tokio::task::spawn_blocking(move || {
        let deck = sim::compile_deck(&entries, &cards);
        let mut rng = StdRng::seed_from_u64(seed);
        sim::simulate(&deck, &params, &mut rng)
    })
    .await
    .map_err(|e| ApiError::BadRequest(format!("simulation failed: {e}")))?;

    Ok(Json(result))
}
