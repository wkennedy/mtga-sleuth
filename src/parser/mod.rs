use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc::Receiver;

use crate::state::AppState;

mod events;
mod extract;

pub use events::{Direction, ParsedEvent};

/// Drive the parser loop: consume log lines, extract events, dispatch to state.
pub async fn run(mut lines: Receiver<String>, state: Arc<AppState>) -> Result<()> {
    let mut accumulator = extract::Accumulator::new();
    while let Some(line) = lines.recv().await {
        for event in accumulator.feed(line) {
            tracing::debug!(kind = %event.kind, dir = ?event.direction, "parsed event");
            if let Err(e) = state.ingest(event).await {
                tracing::warn!(error = %e, "state ingest failed");
            }
        }
    }
    Ok(())
}
