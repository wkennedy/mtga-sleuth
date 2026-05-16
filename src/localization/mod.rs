//! MTGA's deck names (and many other UI strings) are stored as localization
//! *keys* like `?=?Loc/Decks/Precon/Precon_EPPFDN_RG`. The actual human-readable
//! name lives in `Raw_ClientLocalization_<hash>.mtga` — an SQLite database 
//! shipped with the game installation — under the key with the `?=?Loc/` prefix
//! stripped off (so the lookup key is `Decks/Precon/Precon_EPPFDN_RG`).
//!
//! This module loads the en-US column once at startup into a HashMap. Lookups
//! are O(1); strings without the sentinel prefix pass through unchanged.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;

const KEY_SENTINEL: &str = "?=?Loc/";

pub struct LocDb {
    map: HashMap<String, String>,
}

impl LocDb {
    pub fn empty() -> Self {
        Self { map: HashMap::new() }
    }

    /// Load the most recently modified `Raw_ClientLocalization_*.mtga` from
    /// `dir`. On any failure (file missing, permission, schema mismatch),
    /// returns an empty db and logs a warning — names will then surface as
    /// raw keys, which is annoying but not fatal.
    pub async fn load_or_empty(dir: &Path) -> Self {
        match Self::try_load(dir).await {
            Ok(db) => {
                tracing::info!(count = db.map.len(), dir = %dir.display(), "localization db loaded");
                db
            }
            Err(e) => {
                tracing::warn!(error = %e, dir = %dir.display(), "localization db unavailable; deck names will show raw keys");
                Self::empty()
            }
        }
    }

    async fn try_load(dir: &Path) -> Result<Self> {
        let path = locate_file(dir)?;
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .immutable(true);
        let pool = SqlitePool::connect_with(opts).await.with_context(|| format!("opening {}", path.display()))?;
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT Key, enUS FROM Loc")
            .fetch_all(&pool)
            .await
            .context("reading Loc table (schema may have changed)")?;
        pool.close().await;
        let map = rows.into_iter().collect();
        Ok(Self { map })
    }

    /// Translate a localization-key string in place. Anything not prefixed with
    /// the sentinel passes through. If the key is unknown, the original string
    /// is returned unchanged (safer than silently dropping the name).
    pub fn translate<'a>(&'a self, raw: &'a str) -> Cow<'a, str> {
        if let Some(key) = raw.strip_prefix(KEY_SENTINEL) {
            if let Some(v) = self.map.get(key) {
                return Cow::Borrowed(v.as_str());
            }
        }
        Cow::Borrowed(raw)
    }
}

fn locate_file(dir: &Path) -> Result<PathBuf> {
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("Raw_ClientLocalization_") && name.ends_with(".mtga") {
            let mtime = entry.metadata()?.modified()?;
            if newest.as_ref().is_none_or(|(_, t)| mtime > *t) {
                newest = Some((entry.path(), mtime));
            }
        }
    }
    newest.map(|(p, _)| p).context("no Raw_ClientLocalization_*.mtga file in directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(pairs: &[(&str, &str)]) -> LocDb {
        LocDb { map: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    #[test]
    fn translates_sentinel_prefixed_key() {
        let d = db(&[("Decks/Precon/Precon_EPPFDN_RG", "Path of Power")]);
        assert_eq!(d.translate("?=?Loc/Decks/Precon/Precon_EPPFDN_RG"), "Path of Power");
    }

    #[test]
    fn passes_through_human_name() {
        let d = LocDb::empty();
        assert_eq!(d.translate("Simic Flash (Imp)"), "Simic Flash (Imp)");
    }

    #[test]
    fn passes_through_unknown_key() {
        let d = LocDb::empty();
        assert_eq!(d.translate("?=?Loc/Some/Unknown/Thing"), "?=?Loc/Some/Unknown/Thing");
    }
}
