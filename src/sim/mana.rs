//! Mana-cost parsing, land color derivation, and castability checks for the
//! draw simulator.

use crate::cards::Card;

/// Bitmask of mana colors. C is colorless-specific ({C} pips / Wastes), not
/// "generic".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorMask(pub u8);

pub const COLOR_BITS: [(u8, &str); 6] =
    [(1, "W"), (2, "U"), (4, "B"), (8, "R"), (16, "G"), (32, "C")];

impl ColorMask {
    pub fn from_symbol(s: &str) -> Option<ColorMask> {
        COLOR_BITS.iter().find(|(_, sym)| *sym == s).map(|(bit, _)| ColorMask(*bit))
    }

    pub fn union(self, other: ColorMask) -> ColorMask {
        ColorMask(self.0 | other.0)
    }

    pub fn intersects(self, other: ColorMask) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn color_count(self) -> u32 {
        self.0.count_ones()
    }
}

/// A parsed casting cost. `pips` holds one mask per colored pip (hybrid pips
/// carry multiple bits). Phyrexian pips are treated as payable with life and
/// dropped; `{2/W}` takes the generic branch; `{X}` counts as zero.
#[derive(Debug, Clone, Default)]
pub struct ParsedCost {
    pub generic: u8,
    pub pips: Vec<ColorMask>,
    /// Mana value for curve bucketing — Scryfall `cmc` when available (it is
    /// authoritative for `{2/W}`/`{X}` corner cases), else generic + pips.
    pub mana_value: u8,
}

impl ParsedCost {
    /// Lands needed to pay this cost (may differ from mana_value, e.g. {2/W}).
    pub fn lands_needed(&self) -> usize {
        self.generic as usize + self.pips.len()
    }
}

pub fn parse_mana_cost(text: &str, cmc: Option<f32>) -> (ParsedCost, Vec<String>) {
    let mut cost = ParsedCost::default();
    let mut warnings = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        let sym = &rest[start + 1..start + len];
        rest = &rest[start + len + 1..];
        if sym.contains('P') {
            // Phyrexian ({W/P}, {G/W/P}): payable with 2 life — treat as free.
            continue;
        }
        if let Ok(n) = sym.parse::<u8>() {
            cost.generic = cost.generic.saturating_add(n);
        } else if sym == "X" || sym == "Y" || sym == "Z" {
            // X = 0 for castability purposes.
        } else if sym == "S" {
            // Snow: any land pays it here (no snow tracking).
            cost.generic = cost.generic.saturating_add(1);
        } else if sym.contains('/') {
            // Hybrid: {W/B} → one pip with both bits; {2/W} → generic branch.
            if let Some(n) = sym.split('/').find_map(|p| p.parse::<u8>().ok()) {
                cost.generic = cost.generic.saturating_add(n);
            } else {
                let mask = sym
                    .split('/')
                    .filter_map(ColorMask::from_symbol)
                    .fold(ColorMask::default(), ColorMask::union);
                if mask.is_empty() {
                    warnings.push(format!("unrecognized mana symbol {{{sym}}}"));
                } else {
                    cost.pips.push(mask);
                }
            }
        } else if let Some(mask) = ColorMask::from_symbol(sym) {
            cost.pips.push(mask);
        } else {
            warnings.push(format!("unrecognized mana symbol {{{sym}}}"));
        }
    }
    cost.mana_value = match cmc {
        Some(v) if v >= 0.0 => v.round().min(255.0) as u8,
        _ => cost.generic.saturating_add(cost.pips.len() as u8),
    };
    (cost, warnings)
}

/// Colors a land can produce. Fallback chain: Scryfall `produced_mana` (union
/// across faces) → basic land types in the type line → `Add {..}` clauses in
/// oracle text. Empty mask = nothing found; the caller decides the fallback.
pub fn land_produces(card: &Card) -> ColorMask {
    let mut mask = produced_list_mask(card.produced_mana.as_deref());
    if let Some(faces) = &card.card_faces {
        for f in faces {
            mask = mask.union(produced_list_mask(f.produced_mana.as_deref()));
        }
    }
    if !mask.is_empty() {
        return mask;
    }

    if let Some(tl) = &card.type_line {
        mask = mask.union(basic_types_mask(tl));
    }
    if !mask.is_empty() {
        return mask;
    }

    if let Some(text) = &card.oracle_text {
        mask = mask.union(add_clause_mask(text));
    }
    if let Some(faces) = &card.card_faces {
        for f in faces {
            if let Some(text) = &f.oracle_text {
                mask = mask.union(add_clause_mask(text));
            }
        }
    }
    mask
}

fn produced_list_mask(list: Option<&[String]>) -> ColorMask {
    list.unwrap_or_default()
        .iter()
        .filter_map(|s| ColorMask::from_symbol(s))
        .fold(ColorMask::default(), ColorMask::union)
}

fn basic_types_mask(type_line: &str) -> ColorMask {
    const BASICS: [(&str, u8); 6] = [
        ("Plains", 1),
        ("Island", 2),
        ("Swamp", 4),
        ("Mountain", 8),
        ("Forest", 16),
        ("Wastes", 32),
    ];
    BASICS
        .iter()
        .filter(|(name, _)| type_line.contains(name))
        .fold(ColorMask::default(), |m, (_, bit)| m.union(ColorMask(*bit)))
}

/// Collect `{W}`..`{C}` symbols appearing after "Add" on each oracle line.
fn add_clause_mask(text: &str) -> ColorMask {
    let mut mask = ColorMask::default();
    for line in text.lines() {
        if let Some(pos) = line.find("Add ") {
            let (m, _) = parse_mana_cost(&line[pos..], Some(0.0));
            mask = m.pips.iter().fold(mask, |acc, p| acc.union(*p));
        }
    }
    mask
}

/// Can `lands` (each producing one mana of any one of its colors, all
/// untapped) pay `cost`? Exact via Kuhn's augmenting-path bipartite matching
/// of pips → lands — greedy fails on cases like {W}{U} vs [WU-dual, W-only].
pub fn can_pay(cost: &ParsedCost, lands: &[ColorMask]) -> bool {
    if lands.len() < cost.lands_needed() {
        return false;
    }
    if cost.pips.is_empty() {
        return true;
    }
    // land_match[li] = pip index currently assigned to land li
    let mut land_match: Vec<Option<usize>> = vec![None; lands.len()];
    for pi in 0..cost.pips.len() {
        let mut visited = vec![false; lands.len()];
        if !augment(pi, &cost.pips, lands, &mut visited, &mut land_match) {
            return false;
        }
    }
    // Every pip matched (or we'd have returned); leftover lands cover generic
    // because lands.len() >= pips + generic was checked up front.
    true
}

fn augment(
    pi: usize,
    pips: &[ColorMask],
    lands: &[ColorMask],
    visited: &mut [bool],
    land_match: &mut [Option<usize>],
) -> bool {
    for li in 0..lands.len() {
        if visited[li] || !pips[pi].intersects(lands[li]) {
            continue;
        }
        visited[li] = true;
        match land_match[li] {
            None => {
                land_match[li] = Some(pi);
                return true;
            }
            Some(other) => {
                if augment(other, pips, lands, visited, land_match) {
                    land_match[li] = Some(pi);
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: ColorMask = ColorMask(1);
    const U: ColorMask = ColorMask(2);
    const B: ColorMask = ColorMask(4);
    const R: ColorMask = ColorMask(8);
    const G: ColorMask = ColorMask(16);
    const C: ColorMask = ColorMask(32);
    const WU: ColorMask = ColorMask(3);

    fn card(type_line: Option<&str>, oracle: Option<&str>, produced: Option<&[&str]>) -> Card {
        Card {
            arena_id: 1,
            name: "t".into(),
            mana_cost: None,
            type_line: type_line.map(String::from),
            colors: None,
            rarity: None,
            set: None,
            collector_number: None,
            cmc: None,
            image_small: None,
            image_normal: None,
            scryfall_uri: None,
            legalities: None,
            oracle_text: oracle.map(String::from),
            produced_mana: produced.map(|p| p.iter().map(|s| s.to_string()).collect()),
            card_faces: None,
        }
    }

    #[test]
    fn parses_simple_costs() {
        let (c, w) = parse_mana_cost("{1}{W}{W}", Some(3.0));
        assert!(w.is_empty());
        assert_eq!(c.generic, 1);
        assert_eq!(c.pips, vec![W, W]);
        assert_eq!(c.mana_value, 3);
        assert_eq!(c.lands_needed(), 3);
    }

    #[test]
    fn x_counts_as_zero_but_cmc_wins_for_mana_value() {
        let (c, _) = parse_mana_cost("{X}{R}{R}", Some(2.0));
        assert_eq!(c.generic, 0);
        assert_eq!(c.pips, vec![R, R]);
        assert_eq!(c.mana_value, 2);
    }

    #[test]
    fn hybrid_phyrexian_and_two_brid() {
        let (c, _) = parse_mana_cost("{W/B}", Some(1.0));
        assert_eq!(c.pips, vec![ColorMask(1 | 4)]);

        let (c, _) = parse_mana_cost("{G/P}{G/P}", Some(2.0));
        assert!(c.pips.is_empty()); // phyrexian = payable with life
        assert_eq!(c.generic, 0);
        assert_eq!(c.mana_value, 2); // cmc still authoritative

        let (c, _) = parse_mana_cost("{2/W}", Some(3.0));
        assert_eq!(c.generic, 2);
        assert!(c.pips.is_empty());
        assert_eq!(c.lands_needed(), 2); // pays for 2 even though mv is 3
    }

    #[test]
    fn empty_and_unknown_symbols() {
        let (c, w) = parse_mana_cost("", None);
        assert_eq!(c.generic, 0);
        assert!(c.pips.is_empty());
        assert_eq!(c.mana_value, 0);
        assert!(w.is_empty());

        let (_, w) = parse_mana_cost("{T}", None);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn land_produces_prefers_produced_mana() {
        let c = card(Some("Land — Forest"), None, Some(&["B", "G"]));
        assert_eq!(land_produces(&c), B.union(G));
    }

    #[test]
    fn land_produces_falls_back_to_basic_types_then_oracle() {
        let c = card(Some("Basic Land — Forest"), None, None);
        assert_eq!(land_produces(&c), G);

        let c = card(Some("Land"), Some("{T}: Add {W} or {U}."), None);
        assert_eq!(land_produces(&c), WU);

        let c = card(Some("Land"), Some("{T}: Add {C}."), None);
        assert_eq!(land_produces(&c), C);

        let c = card(Some("Land"), Some("Draw a card."), None);
        assert!(land_produces(&c).is_empty());
    }

    #[test]
    fn can_pay_needs_matching_not_greedy() {
        // Greedy assigning the dual to the W pip would strand the U pip.
        let (cost, _) = parse_mana_cost("{W}{U}", Some(2.0));
        assert!(can_pay(&cost, &[WU, W]));
        assert!(!can_pay(&cost, &[W, W]));
    }

    #[test]
    fn can_pay_generic_and_shortfalls() {
        let (cost, _) = parse_mana_cost("{2}{W}{W}", Some(4.0));
        assert!(can_pay(&cost, &[W, W, C, G]));
        assert!(!can_pay(&cost, &[W, C, C, G])); // only one W source
        assert!(!can_pay(&cost, &[W, W, C])); // generic shortfall
        let (free, _) = parse_mana_cost("", Some(0.0));
        assert!(can_pay(&free, &[]));
    }
}
