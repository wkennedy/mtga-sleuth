use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::sync::{broadcast, mpsc};
use tracing_subscriber::EnvFilter;

mod cards;
mod config;
mod db;
mod localization;
mod log_watcher;
mod parser;
mod state;
mod web;

use config::Config;

#[derive(Parser, Debug)]
#[command(version, about = "Local MTG Arena tracker for Linux", long_about = None)]
struct Cli {
    /// Path to MTGA Player.log. Defaults to the Snap-Steam Proton location.
    #[arg(long, env = "MTGA_LOG_PATH")]
    log_path: Option<String>,

    /// Address to bind the web UI on.
    #[arg(long, env = "MTGA_BIND", default_value = "127.0.0.1:7843")]
    bind: String,

    /// Override SQLite database path. Defaults to XDG data dir.
    #[arg(long, env = "MTGA_DB_PATH")]
    db_path: Option<String>,

    /// Skip downloading the Scryfall card bulk on startup if missing.
    #[arg(long)]
    no_card_db: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn")))
        .init();

    let cli = Cli::parse();
    let config = Config::resolve(cli.log_path, cli.db_path)?;
    tracing::info!(?config, "resolved configuration");

    let pool = db::init(&config.db_path).await?;
    let card_db = Arc::new(cards::CardDb::load_or_fetch(&config.card_cache_path, cli.no_card_db).await?);
    let loc_db = Arc::new(localization::LocDb::load_or_empty(&config.mtga_data_dir).await);

    let (event_tx, _) = broadcast::channel(1024);
    let (line_tx, line_rx) = mpsc::channel::<String>(4096);

    // Tail Player.log → mpsc lines
    let watcher_handle = {
        let log_path = config.log_path.clone();
        tokio::spawn(async move {
            if let Err(e) = log_watcher::run(log_path, line_tx).await {
                tracing::error!(error = %e, "log watcher exited");
            }
        })
    };

    // Lines → typed events → state engine + broadcaster
    let state = Arc::new(state::AppState::new(pool.clone(), card_db.clone(), loc_db.clone(), event_tx.clone()));
    let parser_handle = {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = parser::run(line_rx, state).await {
                tracing::error!(error = %e, "parser loop exited");
            }
        })
    };

    // Web UI / API
    let web_handle = {
        let state = state.clone();
        let bind = config.bind_override.clone().unwrap_or(cli.bind);
        tokio::spawn(async move {
            if let Err(e) = web::serve(bind, state).await {
                tracing::error!(error = %e, "web server exited");
            }
        })
    };

    tokio::select! {
        _ = watcher_handle => tracing::warn!("log watcher task ended"),
        _ = parser_handle => tracing::warn!("parser task ended"),
        _ = web_handle => tracing::warn!("web task ended"),
        _ = tokio::signal::ctrl_c() => tracing::info!("shutdown requested"),
    }

    Ok(())
}
