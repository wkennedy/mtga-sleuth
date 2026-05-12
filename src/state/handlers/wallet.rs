//! Persist `InventoryInfo` (wallet, wildcards, vault progress) to the
//! singleton `wallet` row. MTGA emits this inside `StartHook` and updates the
//! `Changes` array on subsequent gem/gold/wildcard movements.

use anyhow::Result;
use serde_json::Value;

use crate::state::AppState;

pub async fn handle(state: &AppState, inventory_info: &Value) -> Result<()> {
    let gold = inventory_info.get("Gold").and_then(|v| v.as_i64()).unwrap_or(0);
    let gems = inventory_info.get("Gems").and_then(|v| v.as_i64()).unwrap_or(0);
    let wcc = inventory_info.get("WildCardCommons").and_then(|v| v.as_i64()).unwrap_or(0);
    let wcu = inventory_info.get("WildCardUnCommons").and_then(|v| v.as_i64()).unwrap_or(0);
    let wcr = inventory_info.get("WildCardRares").and_then(|v| v.as_i64()).unwrap_or(0);
    let wcm = inventory_info.get("WildCardMythics").and_then(|v| v.as_i64()).unwrap_or(0);
    let vault = inventory_info.get("TotalVaultProgress").and_then(|v| v.as_i64()).unwrap_or(0);
    let track = inventory_info.get("wcTrackPosition").and_then(|v| v.as_i64()).unwrap_or(0);
    let boosters = serde_json::to_string(inventory_info.get("Boosters").unwrap_or(&Value::Null))
        .unwrap_or_else(|_| "[]".into());

    sqlx::query(
        "UPDATE wallet SET gold=?, gems=?, wc_common=?, wc_uncommon=?, wc_rare=?, wc_mythic=?,
         vault_progress=?, wc_track_position=?, boosters=?, updated_at=CURRENT_TIMESTAMP
         WHERE id=1",
    )
    .bind(gold)
    .bind(gems)
    .bind(wcc)
    .bind(wcu)
    .bind(wcr)
    .bind(wcm)
    .bind(vault)
    .bind(track)
    .bind(&boosters)
    .execute(&state.pool)
    .await?;

    tracing::info!(
        gold, gems,
        wildcards_c = wcc, wildcards_u = wcu, wildcards_r = wcr, wildcards_m = wcm,
        "wallet updated"
    );
    Ok(())
}
