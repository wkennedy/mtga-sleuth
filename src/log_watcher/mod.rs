use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);

/// Tail Player.log forever, sending each newline-terminated line through `tx`.
///
/// Handles three failure modes:
/// - File doesn't exist yet (game not launched): wait and retry.
/// - File truncated/rotated (MTGA wipes Player.log on each launch): reopen at offset 0.
/// - File grows: read appended bytes line by line.
pub async fn run(path: PathBuf, tx: Sender<String>) -> Result<()> {
    tracing::info!(path = %path.display(), "starting log watcher");

    loop {
        match tail_once(&path, &tx).await {
            Ok(()) => {
                tracing::warn!("tail loop exited cleanly, reopening");
            }
            Err(e) => {
                tracing::warn!(error = %e, "tail loop error, will retry");
            }
        }
        sleep(REOPEN_BACKOFF).await;
    }
}

async fn tail_once(path: &PathBuf, tx: &Sender<String>) -> Result<()> {
    // Wait for the file to exist.
    while !path.exists() {
        tracing::debug!("log file missing, waiting");
        sleep(REOPEN_BACKOFF).await;
    }

    let file = File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
    let initial_len = file.metadata().await?.len();

    // Start from the beginning so we capture the current session's events.
    // For a long-running service we'd seek to end, but matches/decks/inventory
    // events that fired before tracker startup are still useful to ingest.
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(0)).await?;
    let mut position: u64 = 0;
    tracing::info!(size = initial_len, "tailing from beginning of file");

    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            // EOF — check for rotation, then poll for growth.
            let meta = match tokio::fs::metadata(path).await {
                Ok(m) => m,
                Err(_) => return Ok(()), // file vanished
            };
            let len = meta.len();
            if len < position {
                // Truncated or rotated; reopen from start.
                tracing::info!(old_pos = position, new_len = len, "log truncated, reopening");
                return Ok(());
            }
            sleep(POLL_INTERVAL).await;
            continue;
        }
        position += n as u64;
        // Strip trailing \r\n (Wine writes Windows line endings).
        let trimmed = buf.trim_end_matches(['\n', '\r']).to_string();
        if tx.send(trimmed).await.is_err() {
            return Ok(()); // receiver dropped
        }
    }
}
