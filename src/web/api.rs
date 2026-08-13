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
    Json(json!({"ok": true, "service": "mtga-sleuth"}))
}

pub async fn live(State(state): State<Arc<AppState>>) -> Json<Option<LiveMatch>> {
    Json(state.current.read().await.clone())
}

#[derive(Serialize)]
pub struct DeckRow {
    pub deck_id: String,
    pub name: String,
    pub format: Option<String>,
    pub origin: String,
    pub last_updated: String,
    pub wildcards_needed: wildcards::WildcardCost,
    pub total_missing: u32,
    /// WUBRG-ordered color identity for the row's mana pips.
    pub colors: Vec<String>,
    /// Art-crop image URL for the deck tile (proxied through /cdn).
    pub tile_art: Option<String>,
}

/// Resolve a deck's tile art to an art_crop /cdn URL. Prefers the tile card
/// MTGA chose (`DeckTileId`), falling back to the deck's best-rarity card.
fn resolve_tile_art(state: &AppState, tile_card_id: Option<i64>, fallback: Option<u32>) -> Option<String> {
    [tile_card_id.map(|v| v as u32), fallback]
        .into_iter()
        .flatten()
        .find_map(|id| {
            let small = state.cards.get(id)?.image_small.as_deref()?;
            Some(rewrite_image_url(small).replace("/small/", "/art_crop/"))
        })
}

pub async fn list_decks(State(state): State<Arc<AppState>>) -> Result<Json<Vec<DeckRow>>, ApiError> {
    let rows: Vec<(String, String, Option<String>, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT deck_id, name, format, origin, last_updated, tile_card_id FROM decks ORDER BY last_updated DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut costs = wildcards::costs_for_all_decks(&state, &state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|(deck_id, name, format, origin, last_updated, tile_card_id)| {
                let cost = costs.remove(&deck_id).unwrap_or_default();
                DeckRow {
                    name: state.loc.translate(&name).into_owned(),
                    deck_id,
                    format,
                    origin,
                    last_updated,
                    wildcards_needed: cost.wildcards_needed,
                    total_missing: cost.total_missing,
                    colors: cost.colors,
                    tile_art: resolve_tile_art(&state, tile_card_id, cost.tile_fallback),
                }
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
    pub tile_art: Option<String>,
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
    /// Arena-format legality map (see cards::ARENA_FORMATS). Absent when the
    /// card DB has no legality data (old cache) — the UI must then skip
    /// validation rather than call everything illegal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legalities: Option<std::collections::HashMap<String, String>>,
}

pub async fn get_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeckDetail>, ApiError> {
    let head: Option<(String, String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT deck_id, name, format, tile_card_id FROM decks WHERE deck_id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let (deck_id, name, format, tile_card_id) = head.ok_or(ApiError::NotFound)?;
    let name = state.loc.translate(&name).into_owned();

    let cards: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT card_id, quantity, sideboard FROM deck_cards WHERE deck_id = ?")
            .bind(&deck_id)
            .fetch_all(&state.pool)
            .await?;

    let analysis = wildcards::analyze(&state, &state.pool, cards).await?;
    // Fallback tile: the deck's best-rarity mainboard card with an image.
    let fallback = {
        let rank = |r: Option<&str>| match r {
            Some("mythic") => 4u8,
            Some("rare") => 3,
            Some("uncommon") => 2,
            Some("common") => 1,
            _ => 0,
        };
        analysis
            .mainboard
            .iter()
            .filter(|c| c.image_small.is_some())
            .max_by_key(|c| rank(c.rarity.as_deref()))
            .map(|c| c.arena_id)
    };
    let tile_art = resolve_tile_art(&state, tile_card_id, fallback);
    Ok(Json(DeckDetail {
        deck_id,
        name,
        format,
        mainboard: analysis.mainboard,
        sideboard: analysis.sideboard,
        wildcards_needed: analysis.cost,
        unique_missing: analysis.unique_missing,
        total_missing: analysis.total_missing,
        tile_art,
    }))
}

/// GET /api/decks/{id}/export — the deck as Arena-format text, suitable for
/// pasting into MTGA's import dialog (and for re-import here). Cards missing
/// from the card DB fall back to `arena_id,quantity` lines, which our own
/// importer accepts but Arena's does not.
pub async fn export_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let exists: Option<(String,)> = sqlx::query_as("SELECT deck_id FROM decks WHERE deck_id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    exists.ok_or(ApiError::NotFound)?;

    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT card_id, quantity, sideboard FROM deck_cards WHERE deck_id = ? ORDER BY sideboard, rowid",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;

    let mut text = String::from("Deck\n");
    let mut in_sideboard = false;
    for (card_id, qty, sideboard) in rows {
        if sideboard != 0 && !in_sideboard {
            text.push_str("\nSideboard\n");
            in_sideboard = true;
        }
        match state.cards.get(card_id as u32) {
            Some(c) => match (c.set.as_deref(), c.collector_number.as_deref()) {
                (Some(set), Some(num)) => {
                    text.push_str(&format!("{qty} {} ({}) {num}\n", c.name, set.to_uppercase()))
                }
                _ => text.push_str(&format!("{qty} {}\n", c.name)),
            },
            None => text.push_str(&format!("{card_id},{qty}\n")),
        }
    }

    Ok((
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        text,
    ))
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

#[derive(Deserialize)]
pub struct UpdateDeckRequest {
    pub name: String,
    pub format: Option<String>,
    #[serde(default)]
    pub mainboard: Vec<(u32, u32)>,
    #[serde(default)]
    pub sideboard: Vec<(u32, u32)>,
}

/// PUT /api/decks/{id} — replace a user-authored deck's name/format/cards.
/// MTGA-synced decks are read-only here: the game would overwrite local edits
/// on the next sync, so the UI clones them into a `user-` deck instead.
pub async fn update_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDeckRequest>,
) -> Result<Json<CreateDeckResponse>, ApiError> {
    if !id.starts_with("user-") {
        return Err(ApiError::BadRequest(
            "only user-created decks can be edited; clone the deck first".into(),
        ));
    }
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if req.mainboard.is_empty() && req.sideboard.is_empty() {
        return Err(ApiError::BadRequest("deck must contain at least one card".into()));
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT deck_id FROM decks WHERE deck_id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    exists.ok_or(ApiError::NotFound)?;

    // Aggregate duplicate ids so they can't violate the (deck, card, side) PK.
    let dedupe = |pairs: &[(u32, u32)]| {
        let mut m: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for (id, qty) in pairs {
            *m.entry(*id).or_insert(0) += qty;
        }
        m
    };

    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE decks SET name = ?, format = ?, last_updated = datetime('now') WHERE deck_id = ?")
        .bind(name)
        .bind(req.format.as_deref())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM deck_cards WHERE deck_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    for (side, pairs) in [(0i64, dedupe(&req.mainboard)), (1, dedupe(&req.sideboard))] {
        for (card_id, qty) in pairs {
            sqlx::query(
                "INSERT INTO deck_cards (deck_id, card_id, quantity, sideboard) VALUES (?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(card_id as i64)
            .bind(qty as i64)
            .bind(side)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Json(CreateDeckResponse { deck_id: id }))
}

/// DELETE /api/decks/{id} — user-authored decks only.
pub async fn delete_deck(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !id.starts_with("user-") {
        return Err(ApiError::BadRequest("only user-created decks can be deleted".into()));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM deck_cards WHERE deck_id = ?").bind(&id).execute(&mut *tx).await?;
    let res = sqlx::query("DELETE FROM decks WHERE deck_id = ?").bind(&id).execute(&mut *tx).await?;
    tx.commit().await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(Json(json!({"deleted": id})))
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
        legalities: None, // draft views don't validate formats
    }
}

#[derive(Deserialize)]
pub struct CardSearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct CardSearchResult {
    pub arena_id: u32,
    pub name: String,
    pub mana_cost: Option<String>,
    pub cmc: Option<f32>,
    pub type_line: Option<String>,
    pub rarity: Option<String>,
    pub set: Option<String>,
    pub image_small: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legalities: Option<std::collections::HashMap<String, String>>,
    pub owned: u32,
}

/// GET /api/cards?q=bolt — name-substring search over the card DB for the
/// deck editor. One result per card name (newest printing wins), prefix
/// matches ranked before mid-word matches, owned counts joined in.
pub async fn search_cards(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CardSearchQuery>,
) -> Result<Json<Vec<CardSearchResult>>, ApiError> {
    let q = query.q.trim().to_lowercase();
    if q.len() < 2 {
        return Ok(Json(Vec::new()));
    }
    let limit = query.limit.unwrap_or(20).min(50);

    let mut best: std::collections::HashMap<&str, &Card> = std::collections::HashMap::new();
    for c in state.cards.iter() {
        if c.name.to_lowercase().contains(&q) {
            let entry = best.entry(c.name.as_str()).or_insert(c);
            if c.arena_id > entry.arena_id {
                *entry = c;
            }
        }
    }
    let mut cards: Vec<&Card> = best.into_values().collect();
    cards.sort_by(|a, b| {
        let ap = a.name.to_lowercase().starts_with(&q);
        let bp = b.name.to_lowercase().starts_with(&q);
        bp.cmp(&ap).then_with(|| a.name.cmp(&b.name))
    });
    cards.truncate(limit);

    let ids: std::collections::HashSet<i64> = cards.iter().map(|c| c.arena_id as i64).collect();
    let owned = wildcards::fetch_owned(&state.pool, &ids).await?;

    Ok(Json(
        cards
            .into_iter()
            .map(|c| CardSearchResult {
                arena_id: c.arena_id,
                name: c.name.clone(),
                mana_cost: c.mana_cost.clone(),
                cmc: c.cmc,
                type_line: c.type_line.clone(),
                rarity: c.rarity.clone(),
                set: c.set.clone(),
                image_small: c.image_small.as_deref().map(rewrite_image_url),
                legalities: c.legalities.clone(),
                owned: owned.get(&(c.arena_id as i64)).copied().unwrap_or(0),
            })
            .collect(),
    ))
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
