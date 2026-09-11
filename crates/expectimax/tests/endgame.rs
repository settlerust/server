//! Focused checks for the endgame switches: the race leaf, expected hidden victory points,
//! and the late robber and road bias toward the leader. With every switch off the leaf and
//! the candidate shortlists are unchanged; each test pins one switch's mechanism.
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::weights::EndgameConfig;
use settlers_expectimax::v2::{SearchConfig, Weights, analyze};
use settlers_protocol::Command;

fn quick() -> SearchConfig {
    SearchConfig {
        depth: 1,
        samples: 2,
        scenarios: 3,
        inner_scenarios: 2,
        afterstates: 24,
        ..SearchConfig::default()
    }
}

fn act(state: &State, actor: u8, command: &Command) -> Option<Action> {
    match command {
        Command::Act { action } => Some(*action),
        Command::OfferTrade {
            to, give, receive, ..
        } => {
            let trade = Action::Trade {
                other: to[0],
                give: *give,
                receive: *receive,
            };
            apply(state, actor, trade).ok().map(|_| trade)
        }
        _ => None,
    }
}

/// A main-phase position after setup with the board, hands, and points set by hand.
fn base_state() -> State {
    let (mut state, _history) = common::fixture(4);
    let mut steps = 0;
    while !(state.phase == Phase::Main && state.current == 0 && state.turn > 0) && steps < 500 {
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let a = settlers_expectimax::v2::opponent::act(&state, actor, 0);
        state = apply(&state, actor, a).unwrap_or_else(|_| {
            apply(
                &state,
                actor,
                *legal_actions(&state, actor).first().unwrap(),
            )
            .unwrap()
        });
        steps += 1;
    }
    assert_eq!(state.phase, Phase::Main);
    state
}

fn rebalance(s: &mut State) {
    s.bank = Resources(std::array::from_fn(|r| {
        19 - s.players.iter().map(|p| p.resources.0[r]).sum::<u8>()
    }));
}

#[test]
fn endgame_switches_off_reproduce_the_plain_leaf() {
    let state = base_state();
    let plain = settlers_expectimax::v2::leaf::evaluate(&state, 0, &Weights::default());
    let off = settlers_expectimax::v2::leaf::evaluate(
        &state,
        0,
        &Weights {
            endgame: EndgameConfig::default(),
            ..Weights::default()
        },
    );
    assert_eq!(plain, off);
}

#[test]
fn race_leaf_replaces_opponent_terms_when_the_leader_is_close() {
    let mut state = base_state();
    // Seat 1 is two points from victory and can afford a city.
    state.players[1].settlements = 4;
    state.players[1].cities = 2;
    state.players[1].resources = Resources([2, 1, 1, 2, 3]);
    rebalance(&mut state);
    let weights = Weights {
        endgame: EndgameConfig {
            race: true,
            ..EndgameConfig::default()
        },
        ..Weights::default()
    };
    let terms = settlers_expectimax::v2::leaf::evaluate(&state, 0, &weights);
    assert!(terms.0.contains_key("race_margin"));
    assert!(terms.0.contains_key("leader_tempo"));
    assert!(!terms.0.contains_key("opponent_progress"));
    assert!(!terms.0.contains_key("opponent_production"));
    let margin = terms.0["race_margin"];
    let expected =
        (f64::from(points(&state, 0)) - f64::from(public_points(&state, 1))) * weights.point;
    assert!((margin - expected).abs() < 1e-9);
    // The leader holds a city's cost: their tempo is a full point against this seat.
    assert!((terms.0["leader_tempo"] + weights.point).abs() < 1e-9);
    // Far from victory the race terms stay off.
    state.players[1].settlements = 1;
    state.players[1].cities = 0;
    let terms = settlers_expectimax::v2::leaf::evaluate(&state, 0, &weights);
    assert!(terms.0.contains_key("opponent_progress"));
    assert!(!terms.0.contains_key("race_margin"));
    assert!(!terms.0.contains_key("leader_tempo"));
}

#[test]
fn hidden_points_count_sampled_victory_cards() {
    let mut state = base_state();
    // Seat 2 holds two hidden victory cards: public points stay low, full points rise.
    state.players[2].development[4] = 2;
    let plain = settlers_expectimax::v2::leaf::evaluate(&state, 0, &Weights::default());
    let hidden = settlers_expectimax::v2::leaf::evaluate(
        &state,
        0,
        &Weights {
            endgame: EndgameConfig {
                hidden_points: true,
                ..EndgameConfig::default()
            },
            ..Weights::default()
        },
    );
    let weights = Weights::default();
    let delta = hidden.0["opponent_progress"] - plain.0["opponent_progress"];
    // Two more points on the highest opponent score, and on the sum of opponents' scores.
    let expected = 2.0 * weights.opponent_max_point + 2.0 * weights.opponent_sum_point;
    assert!((delta - expected).abs() < 1e-9, "delta {delta}");
}

#[test]
fn expected_next_gain_tracks_affordability_and_the_deck() {
    let mut state = base_state();
    let gain = settlers_expectimax::v2::leaf::expected_next_gain(&state, 1);
    assert!((0.0..=1.0).contains(&gain));
    // A seat that already holds a city's cost gains a full point next turn.
    state.players[1].resources = Resources([0, 0, 0, 2, 3]);
    rebalance(&mut state);
    let gain = settlers_expectimax::v2::leaf::expected_next_gain(&state, 1);
    assert!((gain - 1.0).abs() < 1e-9, "gain {gain}");
    // With no build left, a development purchase is worth the deck's victory-card share.
    let mut buyer = base_state();
    buyer.players[1].settlements = 5;
    buyer.players[1].cities = 4;
    buyer.players[1].resources = Resources([0, 0, 1, 1, 1]);
    rebalance(&mut buyer);
    let vps = f64::from(buyer.deck[4]) / f64::from(buyer.deck_count());
    let gain = settlers_expectimax::v2::leaf::expected_next_gain(&buyer, 1);
    assert!((gain - vps).abs() < 1e-9, "gain {gain} vs vps {vps}");
}

/// A late position with one crafted board: seat 3 leads by points with a city on a low
/// hex, seat 1 has a city on a high hex, and both hold four cards.
fn robber_state() -> State {
    let mut state = base_state();
    let t = topology();
    let pips = |h: u8| f64::from(6u8.saturating_sub(state.layout.number(h).abs_diff(7)));
    // A low-pips hex for the leader's city and a high-pips hex for the other seat's city.
    let mut low = None;
    let mut high = None;
    for h in 0..19u8 {
        if state.layout.resource(h).is_none() || h == state.robber {
            continue;
        }
        if pips(h) <= 2.0 && low.is_none() {
            low = Some(h);
        }
        if pips(h) >= 5.0 && high.is_none() && Some(h) != low {
            high = Some(h);
        }
    }
    let (low, high) = (low.expect("a low hex"), high.expect("a high hex"));
    // Corner vertices whose other touching hexes have the fewest pips.
    let corner = |hex: u8| {
        t.hex_vertices[hex as usize]
            .iter()
            .copied()
            .min_by_key(|&v| {
                t.incident_hexes[v as usize]
                    .iter()
                    .filter(|&&h| h != hex)
                    .map(|&h| state.layout.number(h).abs_diff(7) as u32)
                    .sum::<u32>()
            })
            .unwrap()
    };
    let mut buildings = [0u8; 54];
    buildings[corner(low) as usize] = 3 + 5; // seat 3 city
    buildings[corner(high) as usize] = 1 + 5; // seat 1 city
    state.board = Board::from_public(&buildings, &[None; 72]).unwrap();
    state.robber = (0..19u8)
        .find(|&h| state.layout.resource(h).is_none())
        .unwrap_or(0);
    state.players[1] = Player {
        resources: Resources([1, 1, 1, 1, 0]),
        settlements: 1,
        cities: 1,
        ..Player::default()
    };
    state.players[3] = Player {
        resources: Resources([1, 1, 1, 1, 0]),
        settlements: 4,
        cities: 2,
        ..Player::default()
    };
    state.players[0].resources = Resources::default();
    state.players[2].resources = Resources::default();
    state.turn = 60;
    state.phase = Phase::Robber {
        resume: Resume::Main,
    };
    state.current = 0;
    rebalance(&mut state);
    state
}

#[test]
fn late_robber_bias_targets_the_leader() {
    use settlers_expectimax::v2::search::robber_preference;
    let state = robber_state();
    let control = robber_preference(&state, 0, &Weights::default(), 4);
    let biased = robber_preference(
        &state,
        0,
        &Weights {
            endgame: EndgameConfig {
                block_leader: true,
                ..EndgameConfig::default()
            },
            ..Weights::default()
        },
        4,
    );
    assert!(!control.is_empty() && !biased.is_empty());
    let victim = |a: Action| match a {
        Action::MoveRobber { victim, .. } => victim,
        _ => panic!("robber action expected"),
    };
    // Without the switch the high-pips hex wins; with it, the leader.
    assert_ne!(victim(control[0]), Some(3));
    assert_eq!(victim(biased[0]), Some(3));
    // Early in the game the biased preference equals the control's.
    let mut early = state;
    early.turn = 1;
    let early_biased = robber_preference(
        &early,
        0,
        &Weights {
            endgame: EndgameConfig {
                block_leader: true,
                ..EndgameConfig::default()
            },
            ..Weights::default()
        },
        4,
    );
    assert_eq!(early_biased, control);
}

#[test]
fn late_road_bias_pays_for_a_blocking_road() {
    use settlers_expectimax::v2::search::road_preference;
    // The center hex's consecutive corners c0..c3: their settlement at c0, their only
    // reachable site at c3, and the route c0-c1-c2-c3. Our settlements at c5, u3 (c0's
    // other neighbor) and beside the passage close every other route, and our roads to
    // c1 and c2 make the passage edges legal for us while adding nothing to our own
    // reach, so the plain score never pays for them.
    let mut state = base_state();
    let t = topology();
    let center = (0..19u8)
        .find(|&h| t.hex_centers[h as usize] == [0, 0])
        .expect("center hex");
    let [c0, c1, c2, c3, _c4, c5] = t.hex_vertices[center as usize];
    let beside = |v: u8, avoid: [u8; 2]| {
        t.neighbors[v as usize]
            .iter()
            .copied()
            .find(|&n| n != avoid[0] && n != avoid[1])
    };
    let (a1, a2, u3) = (
        beside(c1, [c0, c2]).expect("c1 has a third neighbor"),
        beside(c2, [c1, c3]).expect("c2 has a third neighbor"),
        beside(c0, [c1, c5]).expect("c0 has a third neighbor"),
    );
    assert!(settlers_expectimax::leaf::site_value(&state, c3) > 0.0);
    let edge_of = |mut m: u8, mut n: u8| {
        if m > n {
            std::mem::swap(&mut m, &mut n);
        }
        t.edges.iter().position(|&[p, w]| p == m && w == n).unwrap() as u8
    };
    let passage = edge_of(c1, c2);
    let mut buildings = [0u8; 54];
    buildings[a1 as usize] = 1; // our settlements close the ring and flank the passage
    buildings[a2 as usize] = 1;
    buildings[u3 as usize] = 1;
    buildings[c5 as usize] = 1;
    buildings[c0 as usize] = 4; // their settlement
    let mut roads = [None; 72];
    roads[edge_of(a1, c1) as usize] = Some(0);
    roads[edge_of(a2, c2) as usize] = Some(0);
    state.board = Board::from_public(&buildings, &roads).unwrap();
    state.players[0] = Player {
        resources: Resources([1, 1, 0, 0, 0]),
        settlements: 4,
        roads: 2,
        ..Player::default()
    };
    state.players[1] = Player::default();
    state.players[2] = Player::default();
    state.players[3] = Player {
        settlements: 4,
        cities: 2,
        ..Player::default()
    };
    state.turn = 60;
    state.phase = Phase::Main;
    state.current = 0;
    rebalance(&mut state);
    let biased_weights = Weights {
        endgame: EndgameConfig {
            block_leader: true,
            ..EndgameConfig::default()
        },
        ..Weights::default()
    };
    let control = road_preference(&state, 0, &Weights::default(), 8);
    let biased = road_preference(&state, 0, &biased_weights, 8);
    let blocking = Action::BuildRoad { edge: passage };
    assert!(
        !control.contains(&blocking),
        "the reach score already pays for the passage"
    );
    assert!(
        biased.contains(&blocking),
        "the switch pays for the passage"
    );
    // The control still expands toward the site itself, and every listed road is legal.
    let toward = Action::BuildRoad {
        edge: edge_of(c2, c3),
    };
    assert!(control.contains(&toward));
    assert!(biased.iter().all(|a| road_allowed(
        &state,
        0,
        match a {
            Action::BuildRoad { edge } => *edge,
            _ => unreachable!(),
        }
    )));
    // Early in the game the passage stays worthless.
    let mut early = state;
    early.turn = 1;
    assert!(
        !road_preference(&early, 0, &biased_weights, 8).contains(&blocking),
        "the bias armed early"
    );
}

#[test]
fn endgame_config_parses_from_seat_json() {
    let config: SearchConfig =
        serde_json::from_str(r#"{"depth":2,"endgame":{"race":true,"hidden_points":true}}"#)
            .unwrap();
    assert!(config.endgame.race);
    assert!(config.endgame.hidden_points);
    assert!(!config.endgame.block_leader);
    assert_eq!(config.endgame.trigger, 2);
    assert_eq!(config.endgame.margin, 1.0);
    assert_eq!(config.endgame.tempo, 1.0);
    assert!((config.endgame.block_from - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(config.endgame.block_bias, 1.0);
    let all: SearchConfig = serde_json::from_str(
        r#"{"endgame":{"race":true,"trigger":3,"margin":0.5,"tempo":2.0,"hidden_points":true,"block_leader":true,"block_from":0.5,"block_bias":2.0}}"#,
    )
    .unwrap();
    assert_eq!(all.endgame.trigger, 3);
    assert_eq!(all.endgame.margin, 0.5);
    assert_eq!(all.endgame.tempo, 2.0);
    assert!(all.endgame.block_leader);
    assert!((all.endgame.block_from - 0.5).abs() < 1e-9);
    assert_eq!(all.endgame.block_bias, 2.0);
}

#[test]
fn all_endgame_switches_finish_a_game_without_stalls() {
    let (mut state, mut history) = common::fixture(4);
    let config = SearchConfig {
        endgame: EndgameConfig {
            race: true,
            hidden_points: true,
            block_leader: true,
            ..EndgameConfig::default()
        },
        ..quick()
    };
    let mut offered = None;
    let mut steps = 0;
    while !matches!(state.phase, Phase::Finished { .. }) {
        steps += 1;
        assert!(steps < 6000, "game did not finish");
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let action = if actor == 0 {
            let view = common::view(&state, actor, history.len() as u64);
            let report = analyze(&view, &history, &config, offered, &|| 0.0).unwrap();
            for c in &report.candidates {
                assert!((c.contributions.total() - c.expected_value).abs() < 1e-6);
            }
            let command = report.candidates[0].command.clone().unwrap();
            if matches!(command, Command::OfferTrade { .. }) {
                offered = Some(state.turn);
            }
            match act(&state, actor, &command) {
                Some(a) => a,
                None => continue,
            }
        } else {
            legal_actions(&state, actor)
                .into_iter()
                .find(|a| !matches!(a, Action::RelocateSettlement { .. }))
                .unwrap()
        };
        let (next, events) = apply_recorded(&state, actor, action).unwrap();
        state = next;
        common::push(&mut history, events);
    }
}
