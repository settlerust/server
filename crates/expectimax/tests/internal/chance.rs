use super::{Config, Search, Statistics, World, outcomes};
use settlers_engine::*;

fn state() -> State {
    let mut state = State::new(Settings::default(), [17; 32]).unwrap();
    state.phase = Phase::Roll;
    state.turn = 1;
    state.current = 0;
    state
}

#[test]
fn collapsed_dice_and_search_children_preserve_exact_probability_mass() {
    let state = state();
    let branches = outcomes(&state, 0, Action::Roll).unwrap();
    assert_eq!(branches.len(), 11);
    let mut total_mass = 0.0;
    for (chance, probability) in &branches {
        let Some(Chance::Dice { first, second }) = chance else {
            panic!("dice branch expected");
        };
        let multiplicity = (1..=6)
            .flat_map(|a| (1..=6).map(move |b| (a, b)))
            .filter(|&(a, b)| a + b == first + second)
            .count();
        assert_eq!(*probability, multiplicity as f64 / 36.0);
        total_mass += probability;
    }
    assert!((total_mass - 1.0).abs() < 1e-12);

    let mut search = Search {
        seat: 0,
        horizon: 6,
        config: Config::default(),
        stats: Statistics::default(),
    };
    let children = search
        .step(&[World { state, weight: 0.3 }], 0, Action::Roll, 11, 0)
        .unwrap();
    assert_eq!(children.len(), 11);
    assert!((children.iter().map(|w| w.weight).sum::<f64>() - 0.3).abs() < 1e-12);
    let seven_mass: f64 = children
        .iter()
        .filter(|w| matches!(w.state.phase, Phase::Robber { .. }))
        .map(|w| w.weight)
        .sum();
    assert!((seven_mass - 0.3 * 6.0 / 36.0).abs() < 1e-12);
    let sampled = search
        .step(&[World { state, weight: 0.3 }], 0, Action::Roll, 1, 0)
        .unwrap();
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].weight, 0.3);
}

#[test]
fn development_and_theft_weight_card_copies_instead_of_card_types() {
    let mut state = state();
    state.phase = Phase::Main;
    state.players[0].resources = Resources::DEVELOPMENT;
    state.deck = [2, 0, 0, 0, 1];
    let draws = outcomes(&state, 0, Action::BuyDevelopment).unwrap();
    assert_eq!(
        draws,
        vec![
            (
                Some(Chance::Development {
                    card: Development::Knight
                }),
                2.0 / 3.0
            ),
            (
                Some(Chance::Development {
                    card: Development::VictoryPoint
                }),
                1.0 / 3.0
            ),
        ]
    );

    let hex = (state.robber + 1) % 19;
    let mut buildings = [0; 54];
    buildings[topology().hex_vertices[hex as usize][0] as usize] = 2;
    state.board = Board::from_public(&buildings, &[None; 72]).unwrap();
    state.players[1].settlements = 1;
    state.players[1].resources = Resources([3, 0, 0, 0, 1]);
    state.phase = Phase::Robber {
        resume: Resume::Main,
    };
    let theft = outcomes(
        &state,
        0,
        Action::MoveRobber {
            hex,
            victim: Some(1),
        },
    )
    .unwrap();
    assert_eq!(
        theft,
        vec![
            (
                Some(Chance::Steal {
                    resource: Resource::Brick
                }),
                0.75
            ),
            (
                Some(Chance::Steal {
                    resource: Resource::Ore
                }),
                0.25
            ),
        ]
    );
}
