use super::super::{Leaf, OpeningMode, RobberConfig, Weights, bargain};
use super::{Cache, Limits, Search};
use settlers_engine::*;

fn pips(number: u8) -> u8 {
    if (2..=12).contains(&number) && number != 7 {
        6 - number.abs_diff(7)
    } else {
        0
    }
}

fn search(robber: RobberConfig, expected_hands: Option<[[f64; 5]; 4]>) -> Search<'static> {
    let limits = Limits {
        depth: 1,
        beam: 4,
        root_beam: 4,
        per_candidate: 1,
        afterstates: 96,
        scenarios: 2,
        inner_scenarios: 1,
        node_budget: 1_000_000,
        leaf_opponents: false,
        opening: OpeningMode::Planner,
        robber,
        expected_hands,
    };
    Search::new(
        0,
        Weights::default(),
        limits,
        Box::leak(Box::new(|| false)) as &'static dyn Fn() -> bool,
        Leaf::Hand,
        Box::leak(Box::new(Cache::default())),
    )
}

/// One layout: seat 1 leads with a city on the best hex and another on a poor one, seat 2 has
/// three settlements on a second best hex, both share a third good hex, and (when wanted)
/// seat 3 has three settlements on a fourth. Returns the state in the robber phase for seat
/// 0, plus the leader's hex, the pip-heavy hex, and the shared hex.
fn crafted(with_third: bool) -> (State, u8, u8, u8) {
    for seed in 1..32u8 {
        let Some(setup) = try_crafted(seed, with_third) else {
            continue;
        };
        return setup;
    }
    panic!("no usable layout in 32 seeds");
}

/// Building vertices for each chosen hex that lie on no other chosen hex, so every hex's
/// score comes from exactly its own buildings. `pools[i]` are candidate hexes and `need[i]`
/// how many free vertices hex i requires.
fn vertex_plan(pools: &[&[u8]], need: &[usize]) -> Option<(Vec<u8>, Vec<Vec<u8>>)> {
    let t = topology();
    let mut assignment: Vec<u8> = Vec::new();
    let solve = |assign: &[u8]| -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for (i, &h) in assign.iter().enumerate() {
            let free: Vec<u8> = t.hex_vertices[h as usize]
                .iter()
                .copied()
                .filter(|&v| {
                    !assign
                        .iter()
                        .enumerate()
                        .filter(|&(j, _)| j != i)
                        .any(|(_, &k)| t.hex_vertices[k as usize].contains(&v))
                })
                .collect();
            if free.len() < need[i] {
                return None;
            }
            out.push(free[..need[i]].to_vec());
        }
        Some(out)
    };
    fn recurse(
        pools: &[&[u8]],
        assign: &mut Vec<u8>,
        solve: &impl Fn(&[u8]) -> Option<Vec<Vec<u8>>>,
    ) -> Option<(Vec<u8>, Vec<Vec<u8>>)> {
        if assign.len() == pools.len() {
            let assign = assign.clone();
            return solve(&assign).map(|plan| (assign, plan));
        }
        for &h in pools[assign.len()] {
            if !assign.contains(&h) {
                assign.push(h);
                if let Some(found) = recurse(pools, assign, solve) {
                    return Some(found);
                }
                assign.pop();
            }
        }
        None
    }
    recurse(pools, &mut assignment, &solve)
}

fn try_crafted(seed: u8, with_third: bool) -> Option<(State, u8, u8, u8)> {
    let mut s = State::new(
        Settings {
            players: 4,
            ..Settings::default()
        },
        [seed; 32],
    )
    .ok()?;
    let t = topology();
    let desert = s.layout.desert();
    let desert_v = t.hex_vertices[desert as usize][0];
    let pool = |want: u8| -> Vec<u8> {
        (0..19u8)
            .filter(|&h| {
                h != desert
                    && s.layout.resource(h).is_some()
                    && pips(s.layout.number(h)) == want
                    && !t.hex_vertices[h as usize].contains(&desert_v)
            })
            .collect()
    };
    let p5 = pool(5);
    let p1 = pool(1);
    // Order: seat 1's city hex, seat 1's second city, seat 2's hex, the shared hex, seat 3's hex.
    let (pools, need): (Vec<&[u8]>, Vec<usize>) = if with_third {
        (
            vec![
                p5.as_slice(),
                p1.as_slice(),
                p5.as_slice(),
                p5.as_slice(),
                p5.as_slice(),
            ],
            vec![1, 1, 3, 2, 3],
        )
    } else {
        (
            vec![p5.as_slice(), p1.as_slice(), p5.as_slice(), p5.as_slice()],
            vec![1, 1, 3, 2],
        )
    };
    let (hexes, plan) = vertex_plan(&pools, &need)?;
    let mut buildings = [0u8; 54];
    buildings[desert_v as usize] = 1; // seat 0 on the desert
    buildings[plan[0][0] as usize] = 6; // seat 1 city on the best hex
    buildings[plan[1][0] as usize] = 6; // seat 1 city on a poor hex
    for i in 0..3 {
        buildings[plan[2][i] as usize] = 3; // seat 2 settlements
    }
    buildings[plan[3][0] as usize] = 2; // seat 1 settlement on the shared hex
    buildings[plan[3][1] as usize] = 3; // seat 2 settlement on the shared hex
    if with_third {
        for i in 0..3 {
            buildings[plan[4][i] as usize] = 4; // seat 3 settlements
        }
    }
    s.board = Board::from_public(&buildings, &[None; 72]).ok()?;
    s.players[0].settlements = 1;
    s.players[0].resources = Resources([0, 0, 2, 2, 0]); // wool and grain: ore is the need
    s.players[1].cities = 2;
    s.players[1].settlements = 1;
    s.players[1].resources = Resources([0, 0, 0, 0, 2]); // two ore
    s.players[2].settlements = 4;
    s.players[2].resources = Resources([0, 0, 0, 7, 0]); // seven grain
    s.players[3].settlements = u8::from(with_third) * 3;
    s.players[3].resources = Resources([0, 0, 0, 0, 4]); // four ore
    s.phase = Phase::Robber {
        resume: Resume::Main,
    };
    s.current = 0;
    s.turn = 30;
    Some((s, hexes[0], hexes[2], hexes[3]))
}

fn hex_of(a: &Action) -> u8 {
    match a {
        Action::MoveRobber { hex, .. } => *hex,
        _ => panic!("not a robber move"),
    }
}
fn victim_of(a: &Action) -> Option<u8> {
    match a {
        Action::MoveRobber { victim, .. } => *victim,
        _ => panic!("not a robber move"),
    }
}
fn score_of(rows: &[(f64, Action)], hex: u8, victim: Option<u8>) -> f64 {
    rows.iter()
        .find(|(_, a)| hex_of(a) == hex && victim_of(a) == victim)
        .map(|(score, _)| *score)
        .unwrap_or_else(|| panic!("expected a row for hex {hex} victim {victim:?}"))
}

#[test]
fn default_scores_rank_the_most_blocked_pips_first() {
    let (s, _h1, h3, _h4) = crafted(false);
    let rows = search(RobberConfig::default(), None).robber_rows(&s, 0);
    let best = rows
        .iter()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("rows exist");
    assert_eq!(
        hex_of(&best.1),
        h3,
        "three settlements on one hex block most"
    );
    let shortlist = search(RobberConfig::default(), None).candidates(&s, 0, 0);
    assert_eq!(shortlist.len(), 4);
    assert!(shortlist.iter().all(|a| hex_of(a) == hex_of(a)));
}

#[test]
fn leader_switch_ranks_the_leaders_own_best_hex_above_the_pip_heaviest() {
    let (s, h1, h3, _h4) = crafted(false);
    let control = search(RobberConfig::default(), None).robber_rows(&s, 0);
    let control_h1 = score_of(&control, h1, Some(1));
    let control_h3 = score_of(&control, h3, Some(2));
    let arm_rows = search(
        RobberConfig {
            leader: true,
            ..RobberConfig::default()
        },
        None,
    )
    .robber_rows(&s, 0);
    let arm_h1 = score_of(&arm_rows, h1, Some(1));
    let arm_h3 = score_of(&arm_rows, h3, Some(2));
    assert!(
        control_h3 > control_h1,
        "the control prefers the pip-heavy rival hex"
    );
    assert!(
        arm_h1 > arm_h3,
        "the arm puts the leader's own best hex above the pip-heavy rival hex"
    );
    let best = arm_rows
        .iter()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("rows exist");
    assert_eq!(hex_of(&best.1), h1);
}

#[test]
fn need_switch_prefers_the_victim_likely_to_hold_the_missing_card() {
    let (s, _h1, _h3, h4) = crafted(false);
    // Seat 1 (two ore in expectation) and seat 2 (seven grain, no ore) both sit on hex h4.
    let mut expected = [[0.0f64; 5]; 4];
    expected[1][4] = 2.0;
    expected[2][3] = 7.0;
    let control = search(RobberConfig::default(), None).robber_rows(&s, 0);
    assert!(
        score_of(&control, h4, Some(2)) > score_of(&control, h4, Some(1)),
        "the control prefers the bigger hand on the shared hex"
    );
    let arm = search(
        RobberConfig {
            need: true,
            ..RobberConfig::default()
        },
        Some(expected),
    )
    .robber_rows(&s, 0);
    assert!(
        score_of(&arm, h4, Some(1)) > score_of(&arm, h4, Some(2)),
        "the arm prefers the victim likely to hold the needed ore"
    );
    // Without a hand gap the arm keeps the hand-size bonus.
    let rich = {
        let mut s = s;
        s.players[0].resources = Resources([3, 3, 3, 3, 3]);
        s
    };
    let fallback = search(
        RobberConfig {
            need: true,
            ..RobberConfig::default()
        },
        Some(expected),
    )
    .robber_rows(&rich, 0);
    assert!(
        score_of(&fallback, h4, Some(2)) > score_of(&fallback, h4, Some(1)),
        "a hand that affords every build keeps the hand-size bonus"
    );
}

#[test]
fn threat_switch_never_steals_from_a_low_threat_partner() {
    let mut found = None;
    for seed in 1..32u8 {
        let Some((s, ..)) = try_crafted(seed, true) else {
            continue;
        };
        let threats = bargain::threats(&s, 0);
        if threats[1].total >= 0.5 && threats[3].total < 0.5 {
            found = Some(s);
            break;
        }
    }
    let s = found.expect("no layout separates the threat from the partner in 32 seeds");
    let control = search(RobberConfig::default(), None).robber_rows(&s, 0);
    assert!(
        control.iter().any(|(_, a)| victim_of(a) == Some(3)),
        "the control still considers robbing seat 3"
    );
    let arm = search(
        RobberConfig {
            threat: true,
            ..RobberConfig::default()
        },
        None,
    )
    .robber_rows(&s, 0);
    assert!(
        arm.iter().all(|(_, a)| victim_of(a) != Some(3)),
        "seat 3 is never robbed while a threatening victim exists"
    );
    assert!(arm.iter().any(|(_, a)| victim_of(a) == Some(1)));
}

#[test]
fn threat_switch_keeps_partner_steals_when_no_safe_victim_exists() {
    // Only a low-threat seat has buildings and cards, so every steal robs a partner.
    for seed in 1..32u8 {
        let Ok(mut s) = State::new(
            Settings {
                players: 4,
                ..Settings::default()
            },
            [seed; 32],
        ) else {
            continue;
        };
        let t = topology();
        let desert = s.layout.desert();
        let desert_v = t.hex_vertices[desert as usize][0];
        let Some(&h3) = (0..19u8)
            .filter(|&h| {
                h != desert
                    && s.layout.resource(h).is_some()
                    && pips(s.layout.number(h)) == 4
                    && !t.hex_vertices[h as usize].contains(&desert_v)
            })
            .collect::<Vec<_>>()
            .first()
        else {
            continue;
        };
        let mut buildings = [0u8; 54];
        buildings[desert_v as usize] = 1;
        buildings[t.hex_vertices[h3 as usize][0] as usize] = 4;
        let Ok(board) = Board::from_public(&buildings, &[None; 72]) else {
            continue;
        };
        s.board = board;
        s.players[0].settlements = 1;
        s.players[3].settlements = 1;
        s.players[3].resources = Resources([0, 0, 0, 0, 3]);
        s.phase = Phase::Robber {
            resume: Resume::Main,
        };
        s.current = 0;
        s.turn = 30;
        if bargain::threats(&s, 0)[3].total >= 0.5 {
            continue;
        }
        let rows = search(
            RobberConfig {
                threat: true,
                ..RobberConfig::default()
            },
            None,
        )
        .robber_rows(&s, 0);
        assert!(
            rows.iter().any(|(_, a)| victim_of(a) == Some(3)),
            "partner steals stay among the rows when no safe victim exists"
        );
        let shortlist = search(
            RobberConfig {
                threat: true,
                ..RobberConfig::default()
            },
            None,
        )
        .candidates(&s, 0, 0);
        assert_eq!(shortlist.len(), 4);
        assert!(shortlist.iter().any(|a| victim_of(a) == Some(3)));
        return;
    }
    panic!("no usable layout in 32 seeds");
}
