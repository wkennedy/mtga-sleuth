use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};

/// Default MTGA Player.log location for Snap Steam + Proton (app id 2141910).
const DEFAULT_LOG_RELATIVE: &str = "snap/steam/common/.local/share/Steam/steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log";

/// Alternate locations checked when the Snap path is missing.
fn fallback_log_paths(home: &Path) -> Vec<PathBuf> {
    let suffix = "steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log";
    vec![
        home.join(format!(".steam/steam/{suffix}")),
        home.join(format!(".local/share/Steam/{suffix}")),
        home.join(format!(".var/app/com.valvesoftware.Steam/data/Steam/{suffix}")),
    ]
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_path: PathBuf,
    pub db_path: PathBuf,
    pub card_cache_path: PathBuf,
    pub bind_override: Option<String>,
}

impl Config {
    pub fn resolve(log_override: Option<String>, db_override: Option<String>) -> Result<Self> {
        let base = BaseDirs::new().context("could not determine HOME directory")?;
        let project = ProjectDirs::from("dev", "mtga-tracker", "mtga-tracker")
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
        let card_cache_path = cache_dir.join("scryfall-arena.json");

        Ok(Self {
            log_path,
            db_path,
            card_cache_path,
            bind_override: None,
        })
    }
}
