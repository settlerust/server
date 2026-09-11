//! Focused tests for the development-card timing switches: `knight_play_value` (leaf),
//! `buy_tempo` (purchase rule), and `hold_knight` (knight timing rule).
#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::leaf;
use settlers_expectimax::v2::search::{Limits, Search};
use settlers_expectimax::v2::{Cache, Leaf, Weights};

fn limits() -> Limits {
    Limits {
        depth: 1,
        beam: 4,
        root_beam: 4,
        per_candidate: 1,
        afterstates: 96,
        scenarios: 4,
        inner_scenarios: 2,
        node_budget: 1_000_000,
        leaf_opponents: false,
        opening: settlers_expectimax::v2::OpeningMode::Planner,
        knight_play_value: false,
        buy_tempo: false,
        hold_knight: false,
    }
}
/// The root candidate list of seat `p` under the given switches, on the true state.
fn candidates(s: &State, p: u8, limits: Limits) -> Vec<Action> {
    let never = || false;
    let mut cache = Cache::default();
    Search::new(
        p,
        Weights::default(),
        limits,
        &never,
        Leaf::Hand,
        &mut cache,
    )
    .candidates(s, p, 0)
}
fn development_term(s: &State, p: u8, w: &Weights, shaped: bool) -> f64 {
    leaf::evaluate_with(s, p, w, shaped).0["development"]
}
/// Seat 0 of the fixture holds settlements on hexes 0, 3, 4, 14, 15, 18; the best by
/// weighted rate is hex 3 (wool 8, five pips, tied with hex 18 which sorts later).
const BEST_HEX: u8 = 3;

#[test]
fn knight_play_value_scales_played_knights_by_army_distance() {
    let (mut state, _) = common::fixture(4);
    let w = Weights::default();
    // Two knights played, nobody near the award: one away from taking it.
    state.players[0].played_knights = 2;
    let flat = development_term(&state, 0, &w, false);
    let shaped = development_term(&state, 0, &w, true);
    assert_eq!(flat, 2.0 * w.knight);
    assert_eq!(shaped, 2.0 * w.knight * 1.75);
    // Far behind an opponent who already holds the award: less than the flat weight.
    state.players[0].played_knights = 1;
    state.players[1].played_knights = 3;
    state.largest_army = Some(1);
    let flat = development_term(&state, 0, &w, false);
    let shaped = development_term(&state, 0, &w, true);
    assert_eq!(flat, w.knight);
    assert_eq!(shaped, w.knight * 0.5);
    // Holding the award: the spent knights keep a defence credit above the flat weight.
    state.players[0].played_knights = 3;
    state.largest_army = Some(0);
    let flat = development_term(&state, 0, &w, false);
    let shaped = development_term(&state, 0, &w, true);
    assert_eq!(flat, 3.0 * w.knight);
    assert_eq!(shaped, 3.0 * w.knight * 1.25);
    // Held cards still count the same under both leaves.
    state.players[0].development[Development::RoadBuilding as usize] = 1;
    let flat = development_term(&state, 0, &w, false);
    let shaped = development_term(&state, 0, &w, true);
    assert_eq!(shaped - flat, 3.0 * w.knight * 0.25);
}

#[test]
fn knight_play_value_credits_a_held_knight_with_the_production_it_unblocks() {
    let (mut state, _) = common::fixture(4);
    let w = Weights::default();
    state.robber = BEST_HEX;
    state.players[0].development[Development::Knight as usize] = 1;
    let flat = development_term(&state, 0, &w, false);
    let shaped = development_term(&state, 0, &w, true);
    // The held knight counts as a card under both leaves; the switch adds one roll of the
    // blocked hex: five pips of wool at the wool resource weight, in card currency.
    let unblock = 5.0 / 36.0 * w.resource_value[Resource::Wool as usize] * w.card_value;
    assert_eq!(flat, w.development);
    assert!((shaped - flat - unblock).abs() < 1e-9);
    // No credit without a playable knight, or when the robber blocks nothing of this seat's.
    state.players[0].development[Development::Knight as usize] = 0;
    assert_eq!(
        development_term(&state, 0, &w, true),
        development_term(&state, 0, &w, false)
    );
    state.players[0].development[Development::Knight as usize] = 1;
    state.robber = state.layout.desert();
    assert_eq!(
        development_term(&state, 0, &w, true),
        development_term(&state, 0, &w, false)
    );
    // A knight bought this turn is not playable yet and earns no unblock credit.
    state.robber = BEST_HEX;
    state.players[0].development[Development::Knight as usize] = 0;
    state.players[0].new_development[Development::Knight as usize] = 1;
    assert_eq!(
        development_term(&state, 0, &w, true),
        development_term(&state, 0, &w, false)
    );
}

#[test]
fn buy_tempo_offers_a_purchase_only_when_no_build_is_within_a_turn() {
    let (mut state, _) = common::fixture(4);
    state.phase = Phase::Main;
    state.current = 0;
    // A city is affordable now: the tempo rule withholds the purchase.
    state.players[0].resources = Resources([0, 0, 1, 2, 3]);
    let control = candidates(&state, 0, limits());
    assert!(control.contains(&Action::BuyDevelopment));
    let mut tempo = limits();
    tempo.buy_tempo = true;
    let treatment = candidates(&state, 0, tempo);
    assert!(!treatment.contains(&Action::BuyDevelopment));
    assert!(treatment.contains(&Action::BuildCity { vertex: 13 }));
    // Nothing is affordable within a turn (seat 0 has no ore and thin brick/lumber), so the
    // purchase stays on the table.
    state.players[0].resources = Resources([0, 0, 1, 1, 1]);
    let treatment = candidates(&state, 0, tempo);
    assert!(treatment.contains(&Action::BuyDevelopment));
    // An empty deck withholds the purchase under either setting.
    state.deck = [0; 5];
    let control = candidates(&state, 0, limits());
    assert!(!control.contains(&Action::BuyDevelopment));
}

#[test]
fn hold_knight_reserves_main_phase_plays_for_the_blocked_best_hex() {
    let (mut state, _) = common::fixture(4);
    state.phase = Phase::Main;
    state.current = 0;
    state.players[0].development[Development::Knight as usize] = 1;
    // The robber is on the desert: the rule holds the knight, the control offers the play.
    state.robber = state.layout.desert();
    let control = candidates(&state, 0, limits());
    assert!(control.contains(&Action::PlayKnight));
    let mut held = limits();
    held.hold_knight = true;
    let treatment = candidates(&state, 0, held);
    assert!(!treatment.contains(&Action::PlayKnight));
    // With the robber on this seat's best hex the main-phase play is offered again.
    state.robber = BEST_HEX;
    let treatment = candidates(&state, 0, held);
    assert!(treatment.contains(&Action::PlayKnight));
    // A lesser hex of this seat's does not release the knight.
    state.robber = 4; // grain 3, two pips, seat 0 holds a settlement here
    let treatment = candidates(&state, 0, held);
    assert!(!treatment.contains(&Action::PlayKnight));
    // The roll phase keeps the before-the-roll choice under the rule.
    state.phase = Phase::Roll;
    state.robber = BEST_HEX;
    let treatment = candidates(&state, 0, held);
    assert_eq!(treatment, vec![Action::Roll, Action::PlayKnight]);
}
