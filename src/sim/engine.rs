//! The per-game Monte Carlo loop: mulligans, Bo1 smoothing, a heuristic
//! pilot (land drops + castability sampling), and result tallying.

use rand::seq::SliceRandom;
use rand::Rng;

use super::compile::{CompiledDeck, SimCard};
use super::mana::{can_pay, ColorMask, COLOR_BITS};
use super::{CardStats, ColorStats, DeckSummaryOut, MulliganDistribution, SimParams, SimResult};

/// Keep a drawn 7 iff its land count is reasonable. Evaluated on the 7 drawn
/// cards at every hand size (London mulligan); at 4 we always keep.
pub(super) fn keep_decision(lands_in_drawn7: u32, hand_size: u8) -> bool {
    hand_size <= 4 || (2..=5).contains(&lands_in_drawn7)
}

struct Tally {
    kept: [u32; 4], // kept7, kept6, kept5, kept4
    drops_through: [u32; 5],
    screw: u32,
    flood: u32,
    curve_out: u32,
    color_on_time: Vec<u32>,          // parallel to deck.colors_needed
    first_cast_hist: Vec<Vec<u32>>,   // [distinct card][turn-1] = games first castable at that turn
    first_drawn_hist: Vec<Vec<u32>>,
    cast_on_curve: Vec<u32>,
}

pub fn run(deck: &CompiledDeck, params: &SimParams, rng: &mut impl Rng) -> SimResult {
    let turns = params.turns.max(1) as usize;
    let n_cards = deck.cards.len();
    let mut tally = Tally {
        kept: [0; 4],
        drops_through: [0; 5],
        screw: 0,
        flood: 0,
        curve_out: 0,
        color_on_time: vec![0; deck.colors_needed.len()],
        first_cast_hist: vec![vec![0; turns]; n_cards],
        first_drawn_hist: vec![vec![0; turns]; n_cards],
        cast_on_curve: vec![0; n_cards],
    };

    // Which mana values 1..=4 the deck can actually curve into.
    let mut has_mv = [false; 5];
    for c in deck.cards.iter().filter(|c| c.cost().is_some()) {
        let mv = c.mana_value() as usize;
        if (1..=4).contains(&mv) {
            has_mv[mv] = true;
        }
    }

    for _ in 0..params.iterations {
        play_one_game(deck, params, rng, turns, &has_mv, &mut tally);
    }

    finalize(deck, params, turns, tally)
}

fn is_land(deck: &CompiledDeck, ci: u16) -> bool {
    deck.cards[ci as usize].is_land()
}

/// Shuffle + draw the opening hand, resolving Bo1 smoothing and London
/// mulligans. Returns (library, hand, kept hand size). Library is drawn from
/// the END (pop); bottomed cards go to index 0.
pub(super) fn opening_hand(
    deck: &CompiledDeck,
    params: &SimParams,
    rng: &mut impl Rng,
) -> (Vec<u16>, Vec<u16>, u8) {
    let mut hand_size = 7u8;
    loop {
        let mut lib = deck.library.clone();
        lib.shuffle(rng);

        if hand_size == 7 && params.bo1_smoothing {
            // Bo1 smoothing approximation: two candidate shuffles, keep the
            // one whose top-7 land count is closest to the deck's land ratio.
            let mut alt = deck.library.clone();
            alt.shuffle(rng);
            let target = 7.0 * deck.land_count as f64 / deck.library.len().max(1) as f64;
            let count = |l: &[u16]| {
                l.iter().rev().take(7).filter(|&&ci| is_land(deck, ci)).count() as f64
            };
            if (count(&alt) - target).abs() < (count(&lib) - target).abs() {
                lib = alt;
            }
        }

        let mut drawn: Vec<u16> = Vec::with_capacity(7);
        for _ in 0..7.min(lib.len()) {
            drawn.push(lib.pop().unwrap());
        }
        let lands = drawn.iter().filter(|&&ci| is_land(deck, ci)).count() as u32;

        if keep_decision(lands, hand_size) {
            // London bottoming: excess lands beyond 3 first, then the
            // highest-mana-value spells.
            let mut n_bottom = (7 - hand_size) as usize;
            let mut bottom: Vec<u16> = Vec::with_capacity(n_bottom);
            while n_bottom > 0
                && drawn.iter().filter(|&&ci| is_land(deck, ci)).count() > 3
            {
                let pos = drawn.iter().rposition(|&ci| is_land(deck, ci)).unwrap();
                bottom.push(drawn.remove(pos));
                n_bottom -= 1;
            }
            while n_bottom > 0 {
                let pos = drawn
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, &ci)| {
                        let c: &SimCard = &deck.cards[ci as usize];
                        if c.is_land() { (0, 0) } else { (1, c.mana_value()) }
                    })
                    .map(|(i, _)| i)
                    .unwrap();
                bottom.push(drawn.remove(pos));
                n_bottom -= 1;
            }
            for ci in bottom {
                lib.insert(0, ci);
            }
            return (lib, drawn, hand_size);
        }
        hand_size -= 1;
    }
}

fn play_one_game(
    deck: &CompiledDeck,
    params: &SimParams,
    rng: &mut impl Rng,
    turns: usize,
    has_mv: &[bool; 5],
    tally: &mut Tally,
) {
    let (mut lib, mut hand, kept) = opening_hand(deck, params, rng);
    tally.kept[(7 - kept) as usize] += 1;

    let n_cards = deck.cards.len();
    let mut first_cast: Vec<Option<u8>> = vec![None; n_cards];
    let mut first_drawn: Vec<Option<u8>> = vec![None; n_cards];
    for &ci in &hand {
        first_drawn[ci as usize].get_or_insert(1);
    }
    let mut committed: Vec<bool> = vec![false; hand.len()];

    let mut lands_in_play: Vec<ColorMask> = Vec::with_capacity(10);
    let mut in_play_union = ColorMask::default();
    let mut all_drops_made = true;
    let mut curve_ok = true;

    let needed_union = deck
        .colors_needed
        .iter()
        .fold(ColorMask::default(), |m, &(bit, _)| m.union(ColorMask(bit)));

    for t in 1..=turns {
        let t8 = t as u8;
        if !(t == 1 && params.on_play) {
            if let Some(ci) = lib.pop() {
                hand.push(ci);
                committed.push(false);
                first_drawn[ci as usize].get_or_insert(t8);
            }
        }

        // Land policy: play the land adding the most still-missing needed
        // colors; tie-break toward lands producing more colors overall.
        let missing = ColorMask(needed_union.0 & !in_play_union.0);
        let land_pos = hand
            .iter()
            .enumerate()
            .filter(|(_, &ci)| is_land(deck, ci))
            .max_by_key(|(_, &ci)| {
                let produces = match deck.cards[ci as usize].kind {
                    super::compile::CardKind::Land { produces } => produces,
                    _ => unreachable!(),
                };
                (ColorMask(produces.0 & missing.0).color_count(), produces.color_count())
            })
            .map(|(i, _)| i);
        match land_pos {
            Some(pos) => {
                let ci = hand.remove(pos);
                committed.remove(pos);
                let produces = match deck.cards[ci as usize].kind {
                    super::compile::CardKind::Land { produces } => produces,
                    _ => unreachable!(),
                };
                lands_in_play.push(produces);
                in_play_union = in_play_union.union(produces);
            }
            None => all_drops_made = false,
        }
        if all_drops_made && t <= 5 {
            tally.drops_through[t - 1] += 1;
        }

        // Castability sampling (counterfactual — nothing is actually cast, so
        // lands stay the only mana and casting can't change future turns).
        for &ci in &hand {
            let idx = ci as usize;
            if first_cast[idx].is_none() {
                if let Some(cost) = deck.cards[idx].cost() {
                    if can_pay(cost, &lands_in_play) {
                        first_cast[idx] = Some(t8);
                    }
                }
            }
        }

        // Curve-out: each turn 1..=4 the deck can curve into needs an
        // uncommitted castable spell of exactly that mana value in hand.
        if curve_ok && (1..=4).contains(&t) && has_mv[t] {
            let pos = hand.iter().enumerate().position(|(i, &ci)| {
                !committed[i]
                    && deck.cards[ci as usize]
                        .cost()
                        .map(|cost| {
                            cost.mana_value as usize == t && can_pay(cost, &lands_in_play)
                        })
                        .unwrap_or(false)
            });
            match pos {
                Some(i) => committed[i] = true,
                None => curve_ok = false,
            }
        }

        for (k, &(bit, first_turn)) in deck.colors_needed.iter().enumerate() {
            if t8 == first_turn && in_play_union.intersects(ColorMask(bit)) {
                tally.color_on_time[k] += 1;
            }
        }

        if t == 3 && lands_in_play.len() < 3 {
            tally.screw += 1;
        }
        if t == 6 {
            let lands_available =
                lands_in_play.len() + hand.iter().filter(|&&ci| is_land(deck, ci)).count();
            let needed = (deck.max_spell_mv as usize).min(6);
            if lands_available >= needed + 2 {
                tally.flood += 1;
            }
        }
    }

    if curve_ok {
        tally.curve_out += 1;
    }

    for idx in 0..n_cards {
        if let Some(t) = first_cast[idx] {
            for turn in (t as usize)..=turns {
                tally.first_cast_hist[idx][turn - 1] += 1;
            }
            let on_curve_turn = deck.cards[idx].mana_value().max(1);
            if t <= on_curve_turn {
                tally.cast_on_curve[idx] += 1;
            }
        }
        if let Some(t) = first_drawn[idx] {
            for turn in (t as usize)..=turns {
                tally.first_drawn_hist[idx][turn - 1] += 1;
            }
        }
    }
}

fn finalize(deck: &CompiledDeck, params: &SimParams, turns: usize, tally: Tally) -> SimResult {
    let n = params.iterations.max(1) as f64;
    let rate = |c: u32| c as f64 / n;

    let mut cards: Vec<CardStats> = deck
        .cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.cost().is_some())
        .map(|(idx, c)| CardStats {
            arena_id: c.arena_id,
            name: c.name.clone(),
            quantity: c.quantity,
            mana_cost: c.mana_cost.clone(),
            mana_value: c.mana_value(),
            p_cast_on_curve: rate(tally.cast_on_curve[idx]),
            p_cast_by: tally.first_cast_hist[idx].iter().map(|&c| rate(c)).collect(),
            p_drawn_by: tally.first_drawn_hist[idx].iter().map(|&c| rate(c)).collect(),
        })
        .collect();
    cards.sort_by(|a, b| a.mana_value.cmp(&b.mana_value).then(a.name.cmp(&b.name)));

    let colors = deck
        .colors_needed
        .iter()
        .enumerate()
        .map(|(k, &(bit, first_turn))| ColorStats {
            color: COLOR_BITS
                .iter()
                .find(|(b, _)| *b == bit)
                .map(|(_, s)| s.to_string())
                .unwrap_or_default(),
            first_needed_turn: first_turn,
            p_on_time: rate(tally.color_on_time[k]),
        })
        .collect();

    let mut assumptions = vec![
        "Lands are the only mana source; ramp, treasures, and mana rocks are not modeled.".to_string(),
        "All lands enter untapped and can produce any one of their colors.".to_string(),
        "Phyrexian pips are paid with life; {2/W}-style costs pay the generic branch; X = 0.".to_string(),
        "Mulligan rule: keep any 7 drawn cards with 2-5 lands (evaluated at hand sizes 7/6/5); always keep at 4. London bottoming keeps 3 lands, then bottoms the highest-cost spells.".to_string(),
        format!(
            "Mana screw = fewer than 3 lands in play at end of turn 3; flood = lands in play + hand at end of turn 6 at least 2 more than needed (max spell cost, capped at 6); curve-out = a castable spell of each mana value 1-4 the deck contains, on time, over {turns} turns."
        ),
        "Modal double-faced cards with a land back face are treated as spells.".to_string(),
    ];
    if params.bo1_smoothing {
        assumptions.push(
            "Bo1 smoothing modeled as best-of-2 candidate hands by land count vs. the deck's land ratio (Arena's real algorithm is unpublished)."
                .to_string(),
        );
    }

    SimResult {
        params: params.clone(),
        deck: DeckSummaryOut {
            total_cards: deck.library.len() as u32,
            lands: deck.land_count,
            distinct_spells: deck.cards.iter().filter(|c| c.cost().is_some()).count() as u32,
            max_spell_mv: deck.max_spell_mv,
        },
        keep7_rate: rate(tally.kept[0]),
        mulligan_distribution: MulliganDistribution {
            kept7: rate(tally.kept[0]),
            kept6: rate(tally.kept[1]),
            kept5: rate(tally.kept[2]),
            kept4: rate(tally.kept[3]),
        },
        land_drops_on_curve: tally.drops_through.iter().map(|&c| rate(c)).collect(),
        screw_rate: rate(tally.screw),
        flood_rate: rate(tally.flood),
        curve_out_rate: rate(tally.curve_out),
        cards,
        colors,
        warnings: deck.warnings.clone(),
        assumptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Card, CardDb};
    use crate::sim::compile::compile_deck;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

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

    fn white_deck() -> CardDb {
        let mut plains = base(1, "Plains");
        plains.type_line = Some("Basic Land — Plains".into());
        let mut savannah_lion = base(2, "Savannah Lions");
        savannah_lion.mana_cost = Some("{W}".into());
        savannah_lion.type_line = Some("Creature — Cat".into());
        savannah_lion.cmc = Some(1.0);
        CardDb::from_cards_for_test(vec![plains, savannah_lion])
    }

    fn params(iterations: u32, smoothing: bool, seed: Option<u64>) -> SimParams {
        SimParams {
            iterations,
            on_play: true,
            bo1_smoothing: smoothing,
            seed,
            turns: 10,
        }
    }

    #[test]
    fn keep_decision_rules() {
        for h in [7u8, 6, 5] {
            assert!(!keep_decision(0, h));
            assert!(!keep_decision(1, h));
            assert!(keep_decision(2, h));
            assert!(keep_decision(5, h));
            assert!(!keep_decision(6, h));
            assert!(!keep_decision(7, h));
        }
        assert!(keep_decision(0, 4));
        assert!(keep_decision(7, 4));
    }

    #[test]
    fn deterministic_with_same_seed() {
        let db = white_deck();
        let deck = compile_deck(&[(1, 24), (2, 36)], &db);
        let p = params(2_000, true, Some(42));
        let mut rng1 = StdRng::seed_from_u64(42);
        let mut rng2 = StdRng::seed_from_u64(42);
        let r1 = run(&deck, &p, &mut rng1);
        let r2 = run(&deck, &p, &mut rng2);
        assert_eq!(
            serde_json::to_string(&r1).unwrap(),
            serde_json::to_string(&r2).unwrap()
        );
    }

    #[test]
    fn opening_hand_matches_hypergeometric() {
        // 24 lands / 60 cards, no mulligans or smoothing involved: the raw
        // 7-card land count must follow the hypergeometric distribution.
        let db = white_deck();
        let deck = compile_deck(&[(1, 24), (2, 36)], &db);
        let mut rng = StdRng::seed_from_u64(7);
        let trials = 200_000usize;
        let mut counts = [0u32; 8];
        let mut lib = deck.library.clone();
        for _ in 0..trials {
            use rand::seq::SliceRandom;
            lib.shuffle(&mut rng);
            let lands = lib.iter().rev().take(7).filter(|&&ci| ci == 0).count();
            counts[lands] += 1;
        }

        // closed-form hypergeometric via ln-gamma-free binomials (small n)
        fn binom(n: u64, k: u64) -> f64 {
            if k > n {
                return 0.0;
            }
            let mut r = 1.0f64;
            for i in 0..k {
                r *= (n - i) as f64 / (i + 1) as f64;
            }
            r
        }
        let total = binom(60, 7);
        for k in 0..=7u64 {
            let expect = binom(24, k) * binom(36, 7 - k) / total;
            let got = counts[k as usize] as f64 / trials as f64;
            assert!(
                (got - expect).abs() < 0.005,
                "k={k}: got {got:.4}, expected {expect:.4}"
            );
        }
    }

    #[test]
    fn smoothing_raises_keep7_rate() {
        let db = white_deck();
        let deck = compile_deck(&[(1, 24), (2, 36)], &db);
        let mut rng = StdRng::seed_from_u64(11);
        let smoothed = run(&deck, &params(20_000, true, None), &mut rng);
        let mut rng = StdRng::seed_from_u64(11);
        let random = run(&deck, &params(20_000, false, None), &mut rng);
        assert!(
            smoothed.keep7_rate > random.keep7_rate,
            "smoothed {} vs random {}",
            smoothed.keep7_rate,
            random.keep7_rate
        );
    }

    #[test]
    fn mono_white_sanity() {
        let db = white_deck();
        let deck = compile_deck(&[(1, 24), (2, 36)], &db);
        let mut rng = StdRng::seed_from_u64(3);
        let r = run(&deck, &params(10_000, false, None), &mut rng);

        assert_eq!(r.deck.total_cards, 60);
        assert_eq!(r.deck.lands, 24);
        // Kept hands always have >= 2 lands (or hand size 4), so the W
        // one-drop is castable on curve almost always when drawn.
        let lion = &r.cards[0];
        assert_eq!(lion.name, "Savannah Lions");
        assert!(lion.p_cast_on_curve > 0.5, "p_cast_on_curve = {}", lion.p_cast_on_curve);
        // W is needed on turn 1 and nearly always there.
        assert_eq!(r.colors.len(), 1);
        assert!(r.colors[0].p_on_time > 0.9);
        // Rates are probabilities and the mulligan distribution sums to 1.
        let mull_sum = r.mulligan_distribution.kept7
            + r.mulligan_distribution.kept6
            + r.mulligan_distribution.kept5
            + r.mulligan_distribution.kept4;
        assert!((mull_sum - 1.0).abs() < 1e-9);
        // Land drops decrease monotonically.
        for w in r.land_drops_on_curve.windows(2) {
            assert!(w[0] >= w[1]);
        }
    }
}
