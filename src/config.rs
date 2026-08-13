use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};

/// Default MTGA Player.log location for Snap Steam + Proton (app id 2141910).
const DEFAULT_LOG_RELATIVE: &str = "snap/steam/common/.local/share/Steam/steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log";

/// Default MTGA install dir (contains the Raw_ClientLocalization SQLite). Snap Steam.
const DEFAULT_DATA_RELATIVE: &str = "snap/steam/common/.local/share/Steam/steamapps/common/MTGA/MTGA_Data/Downloads/Raw";

/// Alternate locations checked when the Snap path is missing.
fn fallback_log_paths(home: &Path) -> Vec<PathBuf> {
    let suffix = "steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log";
    vec![
        home.join(format!(".steam/steam/{suffix}")),
        home.join(format!(".local/share/Steam/{suffix}")),
        home.join(format!(".var/app/com.valvesoftware.Steam/data/Steam/{suffix}")),
    ]
}

/// Same set of Steam roots as `fallback_log_paths`, but pointing at the MTGA
/// install dir that holds `Raw_ClientLocalization_*.mtga`. Used by the loc DB
/// loader; without a hit, deck-name keys like `?=?Loc/...` stay un-translated.
fn fallback_data_paths(home: &Path) -> Vec<PathBuf> {
    let suffix = "steamapps/common/MTGA/MTGA_Data/Downloads/Raw";
    vec![
        home.join(format!(".steam/steam/{suffix}")),
        home.join(format!(".local/share/Steam/{suffix}")),
        home.join(format!(".var/app/com.valvesoftware.Steam/data/Steam/{suffix}")),
        // Lutris default for non-Steam Wine prefixes pointing at MTGA installs.
        home.join("Games/magic-the-gathering-arena/drive_c/Program Files/Wizards of the Coast/MTGA/MTGA_Data/Downloads/Raw"),
    ]
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_path: PathBuf,
    pub db_path: PathBuf,
    pub card_cache_path: PathBuf,
    pub assets_dir: PathBuf,
    pub mtga_data_dir: PathBuf,
    pub bind_override: Option<String>,
}

impl Config {
    pub fn resolve(
        log_override: Option<String>,
        db_override: Option<String>,
        data_dir_override: Option<String>,
    ) -> Result<Self> {
        let base = BaseDirs::new().context("could not determine HOME directory")?;
        let project = ProjectDirs::from("dev", "mtga-sleuth", "mtga-sleuth")
            .context("could not determine XDG project dirs")?;

        let log_path = match log_override {
            Some(p) => PathBuf::from(p),
            None => {
                let primary = base.home_dir().join(DEFAULT_LOG_RELATIVE);
                if primary.exists() {
                    primary
                } else {
                    fallback_log_paths(base.home_dir())
                        .into_iter()
                        .find(|p| p.exists())
                        .unwrap_or(primary)
                }
            }
        };

        let data_dir = project.data_dir().to_path_buf();
        std::fs::create_dir_all(&data_dir).context("creating data dir")?;
        let cache_dir = project.cache_dir().to_path_buf();
        std::fs::create_dir_all(&cache_dir).context("creating cache dir")?;

        let db_path = db_override.map(PathBuf::from).unwrap_or_else(|| data_dir.join("tracker.sqlite"));
        // v2: cards carry legalities + oracle_text. The old cache lacks them, so
        // a new filename forces a one-time refetch; clean the old file up.
        let card_cache_path = cache_dir.join("scryfall-arena-v2.json");
        let _ = std::fs::remove_file(cache_dir.join("scryfall-arena.json"));
        let assets_dir = cache_dir.join("assets");

        let mtga_data_dir = match data_dir_override {
            Some(p) => PathBuf::from(p),
            None => {
                let primary = base.home_dir().join(DEFAULT_DATA_RELATIVE);
                if primary.exists() {
                    primary
                } else {
                    fallback_data_paths(base.home_dir())
                        .into_iter()
                        .find(|p| p.exists())
                        .unwrap_or(primary)
                }
            }
        };

        Ok(Self {
            log_path,
            db_path,
            card_cache_path,
            assets_dir,
            mtga_data_dir,
            bind_override: None,
        })
    }
}
