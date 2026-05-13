//! Application state: receives parsed events, updates the live match snapshot,
//! persists durable rows (decks, collection, matches, drafts), and broadcasts
//! deltas to SSE subscribers.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::cards::CardDb;
use crate::localization::LocDb;
use crate::parser::{Direction, ParsedEvent};

pub mod handlers;

#[derive(Debug, Clone, Serialize)]
pub struct LiveCard {
    pub arena_id: u32,
    pub name: String,
    pub remaining: u32,
    pub original: u32,
    pub cmc: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveMatch {
    pub match_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub deck_id: Option<String>,
    pub deck_name: Option<String>,
    pub library: Vec<LiveCard>,
    pub played: Vec<LiveCard>,
    pub opponent_revealed: Vec<LiveCard>,
    pub player_life: Option<i32>,
    pub opponent_life: Option<i32>,
    pub turn: Option<u32>,
    pub opponent_screen_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveUpdate {
    /// Full snapshot of the current match (or `None` on match end).
    Match(Option<LiveMatch>),
    /// A new event arrived (lightweight signal for UI to refresh aggregates).
    EventTick { kind: String },
    /// Collection or decks changed.
    CollectionUpdated,
    DecksUpdated,
}

pub struct AppState {
    pub pool: SqlitePool,
    pub cards: Arc<CardDb>,
    pub loc: Arc<LocDb>,
    pub current: RwLock<Option<LiveMatch>>,
    pub updates: broadcast::Sender<LiveUpdate>,
    /// Lookup: deck_id → (name, mainboard map of arena_id→qty)
    pub deck_index: RwLock<HashMap<String, DeckSummary>>,
    /// Filesystem cache for /cdn/* assets (mana SVGs + card images). Populated
    /// by `scripts/download_assets.py`; the /cdn route falls back to Scryfall
    /// when a file isn't present, so this can be empty without breaking the UI.
    pub assets_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeckSummary {
    pub deck_id: String,
    pub name: String,
    pub format: Option<String>,
    pub mainboard: HashMap<u32, u32>,
    pub sideboard: HashMap<u32, u32>,
}

impl AppState {
    pub fn new(
        pool: SqlitePool,
        cards: Arc<CardDb>,
        loc: Arc<LocDb>,
        updates: broadcast::Sender<LiveUpdate>,
        assets_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            pool,
            cards,
            loc,
            current: RwLock::new(None),
            updates,
            deck_index: RwLock::new(HashMap::new()),
            assets_dir,
        }
    }

    /// Process one parsed event: persist, dispatch to handler, broadcast.
    pub async fn ingest(&self, event: ParsedEvent) -> Result<()> {
        // Always persist a copy for debugging / replay.
        let dir = match event.direction {
            Direction::Request => "request",
            Direction::Response => "response",
            Direction::Note => "note",
        };
        let payload_text = serde_json::to_string(&event.payload)?;
        let ts = event.timestamp.to_rfc3339();
        sqlx::query("INSERT INTO raw_events (timestamp, kind, direction, payload) VALUES (?, ?, ?, ?)")
            .bind(&ts)
            .bind(&event.kind)
            .bind(dir)
            .bind(&payload_text)
            .execute(&self.pool)
            .await?;

        // Dispatch on event kind. Unknown kinds are fine; they're already saved.
        // MTGA's modern naming is CamelCase no dots (e.g. `DeckUpsertDeckV3`).
        // We use prefix matching so new event variants get handled automatically.
        let kind = event.kind.as_str();
        match kind {
            // Deck-list events: legacy dotted (`Deck.GetDeckListsV3`) and modern
            // (`DeckGetAllPreconDecksV3`, `DeckUpsertDeckV3`, `DeckGetPlayerDecksV3`, ...).
            k if k.starts_with("Deck.") || k.starts_with("Deck") && (k.contains("Deck") || k.contains("Precon")) => {
                handlers::decks::handle(self, &event).await?;
                let _ = self.updates.send(LiveUpdate::DecksUpdated);
            }
            // Collection / wallet — both naming styles.
            k if k.starts_with("Inventory") || k.starts_with("PlayerInventory") || k.starts_with("Wallet") => {
                handlers::collection::handle(self, &event).await?;
                handlers::inventory_changes::handle(self, &event).await?;
                let _ = self.updates.send(LiveUpdate::CollectionUpdated);
            }
            // StartHook is MTGA's launch-time state dump: wallet + user decks.
            "StartHook" => {
                handlers::starthook::handle(self, &event).await?;
                let _ = self.updates.send(LiveUpdate::DecksUpdated);
                let _ = self.updates.send(LiveUpdate::CollectionUpdated);
            }
            // Booster opens, prize grants, anything that hands out cards.
            k if k.contains("Booster") || k.contains("OpenBooster") || k.contains("Prize")
                || k.contains("Reward") || k.contains("Grant") || k.contains("PackOpen")
                || k.contains("MassOpen") =>
            {
                handlers::inventory_changes::handle(self, &event).await?;
                let _ = self.updates.send(LiveUpdate::CollectionUpdated);
            }
            "MatchGameRoomStateChangedEvent" => {
                handlers::match_room::handle(self, &event).await?;
                let snap = self.current.read().await.clone();
                let _ = self.updates.send(LiveUpdate::Match(snap));
            }
            "GreToClientEvent" => {
                handlers::gre::handle(self, &event).await?;
                let snap = self.current.read().await.clone();
                let _ = self.updates.send(LiveUpdate::Match(snap));
            }
            k if k.starts_with("Draft") || k.starts_with("BotDraft") => {
                handlers::draft::handle(self, &event).await?;
            }
            _ => {}
        }
        let _ = self.updates.send(LiveUpdate::EventTick { kind: event.kind.clone() });
        Ok(())
    }
}
