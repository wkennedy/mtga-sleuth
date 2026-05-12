//! Decode `GreToClientEvent` — the in-game state stream.
//!
//! MVP: we only update the live snapshot's life totals, turn number, and the
//! revealed-card lists. Full game-state reconstruction (zones, library order,
//! etc.) is a follow-up — for now the snapshot is enough for a usable deck
//! tracker once we layer in the deck list separately.

use anyhow::Result;
use serde_json::Value;

use crate::parser::ParsedEvent;
use crate::state::{AppState, LiveCard};

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let messages = event
        .payload
        .get("greToClientEvent")
        .and_then(|v| v.get("greToClientMessages"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if messages.is_empty() {
        return Ok(());
    }

    let mut live_guard = state.current.write().await;
    let Some(live) = live_guard.as_mut() else { return Ok(()); };

    for msg in messages {
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match msg_type {
            "GREMessageType_GameStateMessage" => {
                update_from_game_state(live, &msg, &state.cards);
            }
            "GREMessageType_ConnectResp" => {
                tracing::trace!("GRE connect response");
            }
            _ => {}
        }
    }
    Ok(())
}

fn update_from_game_state(live: &mut crate::state::LiveMatch, msg: &Value, cards: &crate::cards::CardDb) {
    let gs = match msg.get("gameStateMessage") {
        Some(g) => g,
        None => return,
    };

    if let Some(turn_info) = gs.get("turnInfo") {
        live.turn = turn_info.get("turnNumber").and_then(|v| v.as_u64()).map(|n| n as u32);
    }

    if let Some(players) = gs.get("players").and_then(|v| v.as_array()) {
        for p in players {
            let seat = p.get("systemSeatNumber").and_then(|v| v.as_u64()).unwrap_or(0);
            let life = p.get("lifeTotal").and_then(|v| v.as_i64()).map(|n| n as i32);
            // Convention: seat 1 = local player.
            if seat == 1 {
                if let Some(l) = life { live.player_life = Some(l); }
            } else if seat == 2 {
                if let Some(l) = life { live.opponent_life = Some(l); }
            }
        }
    }

    // Revealed cards: gameObjects with controllerSeatId != local + visible.
    if let Some(objects) = gs.get("gameObjects").and_then(|v| v.as_array()) {
        let mut revealed: Vec<LiveCard> = Vec::new();
        for obj in objects {
            let visibility = obj.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
            let controller = obj.get("controllerSeatId").and_then(|v| v.as_u64()).unwrap_or(0);
            let grp_id = obj.get("grpId").and_then(|v| v.as_u64()).map(|n| n as u32);
            let Some(grp_id) = grp_id else { continue };
            if controller != 2 { continue; }
            if !matches!(visibility, "Visibility_Public" | "Visibility_Hidden") { continue; }
            if visibility != "Visibility_Public" { continue; }
            let card = cards.get(grp_id);
            revealed.push(LiveCard {
                arena_id: grp_id,
                name: card.map(|c| c.name.clone()).unwrap_or_else(|| format!("Card #{}", grp_id)),
                remaining: 1,
                original: 1,
                cmc: card.and_then(|c| c.cmc).unwrap_or(0.0),
            });
        }
        if !revealed.is_empty() {
            live.opponent_revealed = revealed;
        }
    }
}
