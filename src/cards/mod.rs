//! Scryfall-backed card metadata, keyed by MTGA's `arena_id` (a.k.a. `grpId`).
//!
//! On first launch we pull Scryfall's `default_cards` bulk file, filter it down
//! to entries with an `arena_id`, and persist the result. Subsequent launches
//! load the cached subset directly. The cache is refreshed when older than
//! [`STALE_AFTER`].

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const BULK_INDEX: &str = "https://api.scryfall.com/bulk-data";
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub arena_id: u32,
    pub name: String,
    pub mana_cost: Option<String>,
    pub type_line: Option<String>,
    pub colors: Option<Vec<String>>,
    pub rarity: Option<String>,
    pub set: Option<String>,
    pub collector_number: Option<String>,
    pub cmc: Option<f32>,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
    pub scryfall_uri: Option<String>,
}

pub struct CardDb {
    by_arena_id: HashMap<u32, Card>,
}

impl CardDb {
    pub fn empty() -> Self {
        Self { by_arena_id: HashMap::new() }
    }

    pub fn get(&self, arena_id: u32) -> Option<&Card> {
        self.by_arena_id.get(&arena_id)
    }

    pub async fn load_or_fetch(cache_path: &Path, skip_fetch: bool) -> Result<Self> {
        if cache_path.exists() && !is_stale(cache_path)? {
            tracing::info!(path = %cache_path.display(), "loading cached card db");
            return load_from_cache(cache_path).await;
        }
        if skip_fetch {
            tracing::warn!("--no-card-db set; skipping Scryfall download. Card names will be missing.");
            return Ok(Self::empty());
        }
        match fetch_and_cache(cache_path).await {
            Ok(db) => Ok(db),
            Err(e) => {
                tracing::warn!(error = %e, "Scryfall fetch failed; running with empty card db");
                if cache_path.exists() {
                    // Stale but better than empty.
                    return load_from_cache(cache_path).await;
                }
                Ok(Self::empty())
            }
        }
    }
}

fn is_stale(path: &Path) -> Result<bool> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?;
    let age = SystemTime::now().duration_since(modified).unwrap_or_default();
    Ok(age > STALE_AFTER)
}

async fn load_from_cache(path: &Path) -> Result<CardDb> {
    let bytes = tokio::fs::read(path).await.with_context(|| format!("reading {}", path.display()))?;
    let cards: Vec<Card> = serde_json::from_slice(&bytes).context("decoding cached card db")?;
    let mut by_arena_id = HashMap::with_capacity(cards.len());
    for c in cards {
        by_arena_id.insert(c.arena_id, c);
    }
    tracing::info!(count = by_arena_id.len(), "card db loaded from cache");
    Ok(CardDb { by_arena_id })
}

async fn fetch_and_cache(cache_path: &Path) -> Result<CardDb> {
    let client = reqwest::Client::builder()
        .user_agent("mtga-tracker/0.1 (+https://github.com/local)")
        .build()?;

    tracing::info!("fetching Scryfall bulk-data index");
    let index: BulkIndex = client.get(BULK_INDEX).send().await?.error_for_status()?.json().await?;
    let entry = index
        .data
        .iter()
        .find(|e| e.bulk_type == "default_cards")
        .context("default_cards entry not in bulk index")?;
    tracing::info!(uri = %entry.download_uri, "downloading default_cards bulk");

    let raw: Vec<ScryfallCard> = client
        .get(&entry.download_uri)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    tracing::info!(total = raw.len(), "decoding bulk; filtering to arena cards");

    let mut filtered = Vec::new();
    for c in raw {
        if let Some(arena_id) = c.arena_id {
            filtered.push(Card {
                arena_id,
                name: c.name,
                mana_cost: c.mana_cost,
                type_line: c.type_line,
                colors: c.colors,
                rarity: c.rarity,
                set: c.set,
                collector_number: c.collector_number,
                cmc: c.cmc,
                image_small: c.image_uris.as_ref().and_then(|u| u.small.clone()),
                image_normal: c.image_uris.as_ref().and_then(|u| u.normal.clone()),
                scryfall_uri: c.scryfall_uri,
            });
        }
    }
    tracing::info!(count = filtered.len(), "filtered to arena cards; writing cache");

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let bytes = serde_json::to_vec(&filtered)?;
    tokio::fs::write(cache_path, &bytes).await?;

    let mut by_arena_id = HashMap::with_capacity(filtered.len());
    for c in filtered {
        by_arena_id.insert(c.arena_id, c);
    }
    Ok(CardDb { by_arena_id })
}

#[derive(Deserialize)]
struct BulkIndex {
    data: Vec<BulkEntry>,
}

#[derive(Deserialize)]
struct BulkEntry {
    #[serde(rename = "type")]
    bulk_type: String,
    download_uri: String,
}

#[derive(Deserialize)]
struct ScryfallCard {
    arena_id: Option<u32>,
    name: String,
    mana_cost: Option<String>,
    type_line: Option<String>,
    colors: Option<Vec<String>>,
    rarity: Option<String>,
    set: Option<String>,
    collector_number: Option<String>,
    cmc: Option<f32>,
    image_uris: Option<ScryfallImageUris>,
    scryfall_uri: Option<String>,
}

#[derive(Deserialize)]
struct ScryfallImageUris {
    small: Option<String>,
    normal: Option<String>,
}

