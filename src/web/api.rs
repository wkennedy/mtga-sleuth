use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cards::{rewrite_image_url, Card};
use crate::state::{AppState, LiveMatch};
use crate::web::wildcards;

pub async fn health() -> impl IntoResponse {
    Json(json!({"ok": true, "service": "mtga-tracker"}))
}

pub async fn live(State(state): State<Arc<AppState>>) -> Json<Option<LiveMatch>> {
    Json(state.current.read().await.clone())
}

#[derive(Serialize)]
pub struct DeckRow {
    pub deck_id: String,
    pub name: String,
    pub format: Option<String>,
    pub last_updated: String,
}

pub async fn list_decks(State(state): State<Arc<AppState>>) -> Result<Json<Vec<DeckRow>>, ApiError> {
    let rows: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT deck_id, name, format, last_updated FROM decks ORDER BY last_updated DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(deck_id, name, format, last_updated)| DeckRow {
                deck_id,
                name: state.loc.translate(&name).into_owned(),
                format,
                last_updated,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct DeckDetail {
    pub deck_id: String,
    pub name: String,
    pub format: Option<String>,
    pub mainboard: Vec<DeckCardEntry>,
    pub sideboard: Vec<DeckCardEntry>,
    pub wildcards_needed: wildcards::WildcardCost,
    pub unique_missing: u32,
    pub total_missing: u32,
}

#[derive(Serialize)]
pub struct DeckCardEntry {
    pub arena_id: u32,
    pub quantity: u32,
    pub name: String,
    pub mana_cost: Option<String>,
    pub cmc: Option<f32>,
    pub rarity: Option<String>,
    pub type_line: Option<String>,
    pub image_small: Option<String>,
    pub owned: u32,
    pub missing: u32,
}

pub async fn get_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeckDetail>, ApiError> {
    let head: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT deck_id, name, format FROM decks WHERE deck_id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let (deck_id, name, format) = head.ok_or(ApiError::NotFound)?;
    let name = state.loc.translate(&name).into_owned();

    let cards: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT card_id, quantity, sideboard FROM deck_cards WHERE deck_id = ?")
            .bind(&deck_id)
            .fetch_all(&state.pool)
            .await?;

    let analysis = wildcards::analyze(&state, &state.pool, cards).await?;
    Ok(Json(DeckDetail {
        deck_id,
        name,
        format,
        mainboard: analysis.mainboard,
        sideboard: analysis.sideboard,
        wildcards_needed: analysis.cost,
        unique_missing: analysis.unique_missing,
        total_missing: analysis.total_missing,
    }))
}

#[derive(Deserialize)]
pub struct CreateDeckRequest {
    pub name: String,
    pub format: Option<String>,
    /// Either preformatted `(arena_id, quantity)` pairs…
    #[serde(default)]
    pub mainboard: Vec<(u32, u32)>,
    #[serde(default)]
    pub sideboard: Vec<(u32, u32)>,
    /// …or raw Arena-format text. If `text` is set, mainboard/sideboard are
    /// re-parsed from it and any explicit pairs are ignored.
    pub text: Option<String>,
}

#[derive(Serialize)]
pub struct CreateDeckResponse {
    pub deck_id: String,
}

/// POST /api/decks — create a user-authored deck. IDs are prefixed `user-`
/// so MTGA-emitted decks (which use bare UUIDs) can never collide with them.
pub async fn create_deck(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateDeckRequest>,
) -> Result<Json<CreateDeckResponse>, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }

    let (mainboard, sideboard) = if let Some(text) = req.text.as_deref() {
        let parsed = crate::web::import::parse_arena_decklist(text, &state.cards);
        let mut main: Vec<(u32, u32)> = Vec::new();
        let mut side: Vec<(u32, u32)> = Vec::new();
        for (id, qty, is_side) in parsed.entries {
            if is_side { side.push((id, qty)); } else { main.push((id, qty)); }
        }
        (main, side)
    } else {
        (req.mainboard, req.sideboard)
    };

    if mainboard.is_empty() && sideboard.is_empty() {
        return Err(ApiError::BadRequest("deck must contain at least one card".into()));
    }

    let deck_id = format!(
        "user-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO decks (deck_id, name, format) VALUES (?, ?, ?)")
        .bind(&deck_id)
        .bind(name)
        .bind(req.format.as_deref())
        .execute(&mut *tx)
        .await?;
    for (id, qty) in &mainboard {
        sqlx::query(
            "INSERT INTO deck_cards (deck_id, card_id, quantity, sideboard) VALUES (?, ?, ?, 0)",
        )
        .bind(&deck_id)
        .bind(*id as i64)
        .bind(*qty as i64)
        .execute(&mut *tx)
        .await?;
    }
    for (id, qty) in &sideboard {
        sqlx::query(
            "INSERT INTO deck_cards (deck_id, card_id, quantity, sideboard) VALUES (?, ?, ?, 1)",
        )
        .bind(&deck_id)
        .bind(*id as i64)
        .bind(*qty as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(Json(CreateDeckResponse { deck_id }))
}

#[derive(Serialize)]
pub struct MatchRow {
    pub match_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub opponent_screen_name: Option<String>,
    pub event_name: Option<String>,
    pub won: Option<bool>,
}

pub async fn list_matches(State(state): State<Arc<AppState>>) -> Result<Json<Vec<MatchRow>>, ApiError> {
    let rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT match_id, started_at, ended_at, opponent_screen_name, event_name, won
         FROM matches ORDER BY started_at DESC LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(match_id, started_at, ended_at, opp, event_name, won)| MatchRow {
                match_id,
                started_at,
                ended_at,
                opponent_screen_name: opp,
                event_name,
                won: won.map(|w| w != 0),
            })
            .collect(),
    ))
}

pub async fn get_match(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row: Option<(String, String, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT match_id, started_at, ended_at, opponent_screen_name, event_name, won
         FROM matches WHERE match_id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    let (match_id, started_at, ended_at, opp, event_name, won) = row.ok_or(ApiError::NotFound)?;
    Ok(Json(json!({
        "match_id": match_id,
        "started_at": started_at,
        "ended_at": ended_at,
        "opponent_screen_name": opp,
        "event_name": event_name,
        "won": won.map(|w| w != 0),
    })))
}

#[derive(Serialize)]
pub struct CollectionEntry {
    pub arena_id: u32,
    pub quantity: u32,
    pub name: String,
    pub set: Option<String>,
    pub rarity: Option<String>,
    pub colors: Option<Vec<String>>,
    pub type_line: Option<String>,
    pub mana_cost: Option<String>,
    pub cmc: Option<f32>,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
}

#[derive(Serialize)]
pub struct Wallet {
    pub gold: i64,
    pub gems: i64,
    pub wc_common: i64,
    pub wc_uncommon: i64,
    pub wc_rare: i64,
    pub wc_mythic: i64,
    pub vault_progress: i64,
    pub wc_track_position: i64,
    pub updated_at: String,
}

pub async fn wallet(State(state): State<Arc<AppState>>) -> Result<Json<Wallet>, ApiError> {
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, String) = sqlx::query_as(
        "SELECT gold, gems, wc_common, wc_uncommon, wc_rare, wc_mythic,
                vault_progress, wc_track_position, updated_at FROM wallet WHERE id = 1",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(Wallet {
        gold: row.0, gems: row.1,
        wc_common: row.2, wc_uncommon: row.3, wc_rare: row.4, wc_mythic: row.5,
        vault_progress: row.6, wc_track_position: row.7, updated_at: row.8,
    }))
}

pub async fn collection(State(state): State<Arc<AppState>>) -> Result<Json<Vec<CollectionEntry>>, ApiError> {
    let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT card_id, quantity FROM collection ORDER BY card_id")
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(card_id, qty)| {
                let c = state.cards.get(card_id as u32);
                CollectionEntry {
                    arena_id: card_id as u32,
                    quantity: qty as u32,
                    name: c.map(|c| c.name.clone()).unwrap_or_else(|| format!("Card #{card_id}")),
                    set: c.and_then(|c| c.set.clone()),
                    rarity: c.and_then(|c| c.rarity.clone()),
                    colors: c.and_then(|c| c.colors.clone()),
                    type_line: c.and_then(|c| c.type_line.clone()),
                    mana_cost: c.and_then(|c| c.mana_cost.clone()),
                    cmc: c.and_then(|c| c.cmc),
                    image_small: c.and_then(|c| c.image_small.as_deref().map(rewrite_image_url)),
                    image_normal: c.and_then(|c| c.image_normal.as_deref().map(rewrite_image_url)),
                }
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct DraftRow {
    pub draft_id: String,
    pub set_code: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub picks: u32,
}

pub async fn list_drafts(State(state): State<Arc<AppState>>) -> Result<Json<Vec<DraftRow>>, ApiError> {
    let rows: Vec<(String, Option<String>, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT d.draft_id, d.set_code, d.started_at, d.completed_at,
                COALESCE((SELECT COUNT(*) FROM draft_picks p WHERE p.draft_id = d.draft_id), 0) as picks
         FROM drafts d ORDER BY d.started_at DESC LIMIT 50",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(draft_id, set_code, started_at, completed_at, picks)| DraftRow {
                draft_id,
                set_code,
                started_at,
                completed_at,
                picks: picks as u32,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct DraftPick {
    pub pack_number: u32,
    pub pick_number: u32,
    pub picked: DeckCardEntry,
    pub pack: Vec<DeckCardEntry>,
}

pub async fn get_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<DraftPick>>, ApiError> {
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "SELECT pack_number, pick_number, picked_card_id, pack_card_ids
         FROM draft_picks WHERE draft_id = ? ORDER BY pack_number, pick_number",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    let picks = rows
        .into_iter()
        .map(|(pack_n, pick_n, picked_id, pack_json)| {
            let pack_ids: Vec<u32> = serde_json::from_str(&pack_json).unwrap_or_default();
            DraftPick {
                pack_number: pack_n as u32,
                pick_number: pick_n as u32,
                picked: card_to_entry(&state, picked_id as u32, 1),
                pack: pack_ids.into_iter().map(|id| card_to_entry(&state, id, 1)).collect(),
            }
        })
        .collect();
    Ok(Json(picks))
}

fn card_to_entry(state: &AppState, arena_id: u32, qty: u32) -> DeckCardEntry {
    let c = state.cards.get(arena_id);
    DeckCardEntry {
        arena_id,
        quantity: qty,
        name: c.map(|c| c.name.clone()).unwrap_or_else(|| format!("Card #{arena_id}")),
        mana_cost: c.and_then(|c| c.mana_cost.clone()),
        cmc: c.and_then(|c| c.cmc),
        rarity: c.and_then(|c| c.rarity.clone()),
        type_line: c.and_then(|c| c.type_line.clone()),
        image_small: c.and_then(|c| c.image_small.as_deref().map(rewrite_image_url)),
        owned: 0,
        missing: 0,
    }
}

pub async fn get_card(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
) -> Result<Json<Card>, ApiError> {
    let mut card = state.cards.get(id).cloned().ok_or(ApiError::NotFound)?;
    card.image_small = card.image_small.as_deref().map(rewrite_image_url);
    card.image_normal = card.image_normal.as_deref().map(rewrite_image_url);
    Ok(Json(card))
}

#[derive(Deserialize)]
pub struct EventsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    pub kind: Option<String>,
}

fn default_limit() -> i64 { 100 }

pub async fn recent_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let limit = q.limit.clamp(1, 1000);
    let rows: Vec<(i64, String, String, String, String)> = if let Some(kind) = q.kind.as_ref() {
        sqlx::query_as(
            "SELECT id, timestamp, kind, direction, payload FROM raw_events WHERE kind = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(kind)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as("SELECT id, timestamp, kind, direction, payload FROM raw_events ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&state.pool)
            .await?
    };
    Ok(Json(
        rows.into_iter()
            .map(|(id, ts, kind, dir, payload)| {
                json!({
                    "id": id,
                    "timestamp": ts,
                    "kind": kind,
                    "direction": dir,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                })
            })
            .collect(),
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match &self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            ApiError::Sqlx(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };
        (status, Json(json!({"error": msg}))).into_response()
    }
}
