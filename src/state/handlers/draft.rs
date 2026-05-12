//! Decode draft pack/pick events.
//!
//! MTGA emits drafts as `Draft.MakePick` (pick made) and pack-state events
//! containing `packCards` (card ids in current pack), `packNumber`, `pickNumber`,
//! and `draftId`. Bot drafts use `BotDraft_DraftPick` and `BotDraft_DraftStatus`.

use anyhow::Result;
use serde_json::Value;

use crate::parser::ParsedEvent;
use crate::state::AppState;

pub async fn handle(state: &AppState, event: &ParsedEvent) -> Result<()> {
    let p = &event.payload;
    let Some(draft_id) = first_str(p, &["draftId", "DraftId", "id"]) else {
        return Ok(());
    };
    let pack_number = first_u32(p, &["packNumber", "PackNumber", "currentPack"]);
    let pick_number = first_u32(p, &["pickNumber", "PickNumber", "currentPick"]);
    let pack_cards = first_array(p, &["packCards", "PackCards", "draftPack"]);
    let picked = first_u32(p, &["pickedCardId", "cardId", "CardId"]);
    let set_code = first_str(p, &["setCode", "SetCode", "set"]);

    sqlx::query(
        "INSERT INTO drafts (draft_id, set_code, started_at) VALUES (?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(draft_id) DO UPDATE SET set_code = COALESCE(excluded.set_code, drafts.set_code)",
    )
    .bind(&draft_id)
    .bind(set_code.as_deref())
    .execute(&state.pool)
    .await?;

    if let (Some(pack_n), Some(pick_n), Some(picked_id)) = (pack_number, pick_number, picked) {
        let cards_json = serde_json::to_string(&pack_cards.unwrap_or_default()).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO draft_picks (draft_id, pack_number, pick_number, picked_card_id, pack_card_ids)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&draft_id)
        .bind(pack_n as i64)
        .bind(pick_n as i64)
        .bind(picked_id as i64)
        .bind(cards_json)
        .execute(&state.pool)
        .await?;
        tracing::info!(draft_id, pack_n, pick_n, picked_id, "draft pick recorded");
    }

    Ok(())
}

fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn first_u32(v: &Value, keys: &[&str]) -> Option<u32> {
    for k in keys {
        if let Some(n) = v.get(*k).and_then(|x| x.as_u64()) {
            return u32::try_from(n).ok();
        }
    }
    None
}

fn first_array(v: &Value, keys: &[&str]) -> Option<Vec<u32>> {
    for k in keys {
        if let Some(arr) = v.get(*k).and_then(|x| x.as_array()) {
            return Some(arr.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect());
        }
    }
    None
}
