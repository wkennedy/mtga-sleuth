//! Compiles a mainboard (arena_id, quantity) list + CardDb into the flat
//! structures the simulation engine iterates over.

use crate::cards::CardDb;

use super::mana::{land_produces, parse_mana_cost, ColorMask, ParsedCost, COLOR_BITS};

#[derive(Debug, Clone)]
pub enum CardKind {
    Land { produces: ColorMask },
    Spell { cost: ParsedCost },
    /// Not in the card DB: still shuffled into the library (it dilutes draws)
    /// but excluded from castability stats.
    UnknownSpell,
}

#[derive(Debug, Clone)]
pub struct SimCard {
    pub arena_id: u32,
    pub name: String,
    pub mana_cost: String,
    pub quantity: u32,
    pub kind: CardKind,
}

impl SimCard {
    pub fn is_land(&self) -> bool {
        matches!(self.kind, CardKind::Land { .. })
    }

    pub fn mana_value(&self) -> u8 {
        match &self.kind {
            CardKind::Spell { cost } => cost.mana_value,
            _ => 0,
        }
    }

    pub fn cost(&self) -> Option<&ParsedCost> {
        match &self.kind {
            CardKind::Spell { cost } => Some(cost),
            _ => None,
        }
    }
}

pub struct CompiledDeck {
    /// Distinct cards; `library` holds indices into this.
    pub cards: Vec<SimCard>,
    /// One entry per physical card (indices into `cards`).
    pub library: Vec<u16>,
    pub land_count: u32,
    /// Highest spell mana value, capped at 7 (for flood math).
    pub max_spell_mv: u8,
    /// (color bit, first turn it's needed): per color, the minimum mana value
    /// among spells with a mono-color pip of that color.
    pub colors_needed: Vec<(u8, u8)>,
    pub warnings: Vec<String>,
}

fn is_land_type(type_line: &str) -> bool {
    type_line.split(|c: char| !c.is_alphanumeric()).any(|w| w == "Land")
}

pub fn compile_deck(entries: &[(u32, u32)], db: &CardDb) -> CompiledDeck {
    let mut cards: Vec<SimCard> = Vec::with_capacity(entries.len());
    let mut warnings = Vec::new();
    let mut mdfc_back_lands: Vec<String> = Vec::new();

    for &(arena_id, quantity) in entries {
        if quantity == 0 {
            continue;
        }
        let Some(card) = db.get(arena_id) else {
            warnings.push(format!("Unknown card {arena_id} ({quantity}x): excluded from castability stats"));
            cards.push(SimCard {
                arena_id,
                name: format!("Unknown #{arena_id}"),
                mana_cost: String::new(),
                quantity,
                kind: CardKind::UnknownSpell,
            });
            continue;
        };

        // Land-ness is judged on the FRONT face only; a spell//land MDFC is
        // played as a spell here (documented limitation).
        let front_type = card
            .type_line
            .as_deref()
            .map(|t| t.split("//").next().unwrap_or(t).trim().to_string())
            .or_else(|| {
                card.card_faces
                    .as_ref()
                    .and_then(|f| f.first())
                    .and_then(|f| f.type_line.clone())
            })
            .unwrap_or_default();

        let kind = if is_land_type(&front_type) {
            let produces = land_produces(card);
            if produces.is_empty() {
                warnings.push(format!("{}: no mana colors detected; treating as colorless source", card.name));
                CardKind::Land { produces: ColorMask(32) }
            } else {
                CardKind::Land { produces }
            }
        } else {
            let has_land_back = card
                .card_faces
                .as_ref()
                .map(|faces| {
                    faces.iter().skip(1).any(|f| {
                        f.type_line.as_deref().map(is_land_type).unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if has_land_back {
                mdfc_back_lands.push(card.name.clone());
            }
            // MDFC/adventure fronts often have an empty top-level mana_cost;
            // fall back to the first face's cost.
            let cost_text = card
                .mana_cost
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    card.card_faces
                        .as_ref()
                        .and_then(|f| f.first())
                        .and_then(|f| f.mana_cost.clone())
                })
                .unwrap_or_default();
            let (cost, cost_warnings) = parse_mana_cost(&cost_text, card.cmc);
            for w in cost_warnings {
                warnings.push(format!("{}: {}", card.name, w));
            }
            CardKind::Spell { cost }
        };

        cards.push(SimCard {
            arena_id,
            name: card.name.clone(),
            mana_cost: card.mana_cost.clone().unwrap_or_default(),
            quantity,
            kind,
        });
    }

    if !mdfc_back_lands.is_empty() {
        warnings.push(format!(
            "{} modal back-face land(s) treated as spells: {}",
            mdfc_back_lands.len(),
            mdfc_back_lands.join(", ")
        ));
    }

    let mut library = Vec::new();
    for (i, c) in cards.iter().enumerate() {
        for _ in 0..c.quantity {
            library.push(i as u16);
        }
    }

    let land_count = cards.iter().filter(|c| c.is_land()).map(|c| c.quantity).sum();
    let max_spell_mv = cards
        .iter()
        .filter(|c| !c.is_land())
        .map(|c| c.mana_value())
        .max()
        .unwrap_or(0)
        .min(7);

    let mut colors_needed = Vec::new();
    for (bit, _) in COLOR_BITS {
        let min_mv = cards
            .iter()
            .filter_map(|c| c.cost())
            .filter(|cost| cost.pips.iter().any(|p| p.0 == bit))
            .map(|cost| cost.mana_value)
            .min();
        if let Some(mv) = min_mv {
            colors_needed.push((bit, mv.max(1)));
        }
    }

    CompiledDeck { cards, library, land_count, max_spell_mv, colors_needed, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Card, CardFace};

    fn db_with(cards: Vec<Card>) -> CardDb {
        CardDb::from_cards_for_test(cards)
    }

    fn base(arena_id: u32, name: &str) -> Card {
        Card {
            arena_id,
            name: name.into(),
            mana_cost: None,
            type_line: None,
            colors: None,
            rarity: None,
            set: None,
            collector_number: None,
            cmc: None,
            image_small: None,
            image_normal: None,
            scryfall_uri: None,
            legalities: None,
            oracle_text: None,
            produced_mana: None,
            card_faces: None,
        }
    }

    fn spell(arena_id: u32, name: &str, mana_cost: &str, cmc: f32) -> Card {
        let mut c = base(arena_id, name);
        c.mana_cost = Some(mana_cost.into());
        c.type_line = Some("Instant".into());
        c.cmc = Some(cmc);
        c
    }

    fn land(arena_id: u32, name: &str, produced: &[&str]) -> Card {
        let mut c = base(arena_id, name);
        c.type_line = Some("Land".into());
        c.produced_mana = Some(produced.iter().map(|s| s.to_string()).collect());
        c
    }

    #[test]
    fn compiles_lands_spells_and_unknowns() {
        let db = db_with(vec![
            land(1, "Plains Proxy", &["W"]),
            spell(2, "Bolt-ish", "{1}{R}", 2.0),
        ]);
        let deck = compile_deck(&[(1, 24), (2, 4), (999, 2)], &db);
        assert_eq!(deck.library.len(), 30);
        assert_eq!(deck.land_count, 24);
        assert_eq!(deck.max_spell_mv, 2);
        assert_eq!(deck.colors_needed, vec![(8, 2)]); // R needed by turn 2
        assert!(deck.warnings.iter().any(|w| w.contains("Unknown card 999")));
        assert!(matches!(deck.cards[2].kind, CardKind::UnknownSpell));
    }

    #[test]
    fn mdfc_front_land_is_land_back_land_is_spell() {
        let mut front_land = base(10, "Land // Spell");
        front_land.type_line = Some("Land // Sorcery".into());
        front_land.produced_mana = Some(vec!["G".into()]);

        let mut back_land = base(11, "Smashing // Pass");
        back_land.type_line = Some("Sorcery // Land".into());
        back_land.cmc = Some(2.0);
        back_land.card_faces = Some(vec![
            CardFace {
                mana_cost: Some("{X}{R}{R}".into()),
                type_line: Some("Sorcery".into()),
                produced_mana: None,
                oracle_text: None,
            },
            CardFace {
                mana_cost: Some(String::new()),
                type_line: Some("Land".into()),
                produced_mana: Some(vec!["R".into()]),
                oracle_text: None,
            },
        ]);

        let db = db_with(vec![front_land, back_land]);
        let deck = compile_deck(&[(10, 4), (11, 4)], &db);
        assert!(deck.cards[0].is_land());
        assert!(!deck.cards[1].is_land());
        // back-face spell picks up the front face's cost (empty top-level)
        let cost = deck.cards[1].cost().unwrap();
        assert_eq!(cost.pips.len(), 2);
        assert!(deck.warnings.iter().any(|w| w.contains("modal back-face")));
        assert_eq!(deck.land_count, 4);
    }

    #[test]
    fn land_with_no_detectable_colors_defaults_to_colorless() {
        let mut l = base(20, "Mystery Land");
        l.type_line = Some("Land".into());
        let db = db_with(vec![l]);
        let deck = compile_deck(&[(20, 1)], &db);
        match deck.cards[0].kind {
            CardKind::Land { produces } => assert_eq!(produces, ColorMask(32)),
            _ => panic!("expected land"),
        }
        assert!(deck.warnings.iter().any(|w| w.contains("Mystery Land")));
    }
}
