//! Monte Carlo deck draw simulator.
//!
//! Compiles a mainboard against the card DB (`compile`), then plays many
//! solitaire games with a heuristic pilot (`engine`): London mulligans,
//! optional Bo1 hand smoothing, one land drop per turn, and castability
//! sampling against what the lands in play can actually produce (`mana`).
//! Nothing is persisted; results are returned as JSON-ready structs.

pub mod compile;
pub mod engine;
pub mod mana;

use serde::Serialize;

pub use compile::compile_deck;
pub use engine::run as simulate;

#[derive(Debug, Clone, Serialize)]
pub struct SimParams {
    pub iterations: u32,
    pub on_play: bool,
    pub bo1_smoothing: bool,
    /// Effective RNG seed — always set by the caller so runs are reproducible.
    pub seed: Option<u64>,
    pub turns: u8,
}

#[derive(Debug, Serialize)]
pub struct SimResult {
    pub params: SimParams,
    pub deck: DeckSummaryOut,
    pub keep7_rate: f64,
    pub mulligan_distribution: MulliganDistribution,
    /// P(made every land drop through turn t), t = 1..=5.
    pub land_drops_on_curve: Vec<f64>,
    pub screw_rate: f64,
    pub flood_rate: f64,
    pub curve_out_rate: f64,
    /// Spells only, sorted by mana value then name.
    pub cards: Vec<CardStats>,
    pub colors: Vec<ColorStats>,
    pub warnings: Vec<String>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeckSummaryOut {
    pub total_cards: u32,
    pub lands: u32,
    pub distinct_spells: u32,
    pub max_spell_mv: u8,
}

#[derive(Debug, Serialize)]
pub struct MulliganDistribution {
    pub kept7: f64,
    pub kept6: f64,
    pub kept5: f64,
    pub kept4: f64,
}

#[derive(Debug, Serialize)]
pub struct CardStats {
    pub arena_id: u32,
    pub name: String,
    pub quantity: u32,
    pub mana_cost: String,
    pub mana_value: u8,
    /// P(first castable no later than its mana value's turn).
    pub p_cast_on_curve: f64,
    /// Cumulative P(castable by turn t), t = 1..=turns.
    pub p_cast_by: Vec<f64>,
    /// Cumulative P(at least one copy drawn by turn t), t = 1..=turns.
    pub p_drawn_by: Vec<f64>,
}

#[derive(Debug, Serialize)]
pub struct ColorStats {
    pub color: String,
    pub first_needed_turn: u8,
    pub p_on_time: f64,
}
