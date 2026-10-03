//! Action ordering provides deterministic priorities before search pruning.
use super::*;
/// The points leader to slow down, when the late-game endgame switch is armed: past
/// `block_from` of the estimated game length, the opponent with the most points.
pub fn block_leader(s: &State, p: u8, w: &Weights) -> Option<u8> {
    if !w.endgame.block_leader {
        return None;
    }
    let length = w.endgame.block_from * w.turns_per_player * f64::from(s.settings.players);
    if f64::from(s.turn) < length {
        return None;
    }
    super::super::leaf::leader_seat(s, p, w)
}
/// Roads worth considering for seat `p`: those that improve its own reach, and with the
/// late endgame switch on, those that lengthen the leader's route to the best site they
/// contest with this seat.
/// Shared measurements for scoring candidate roads: the seat's current reach, the trail
/// race, the contested site a blocking road could take, and the leading opponent's
/// reachable value when the `opponent_expansion` term is on.
pub(super) struct RoadBase {
    base_best: f64,
    contend: bool,
    own_length: u8,
    deny: Option<(u8, u8)>,
    their_route: Option<[u8; 54]>,
    base_opp: f64,
}
pub(super) fn road_base(s: &State, p: u8, w: &Weights) -> RoadBase {
    let own = s.players[p as usize];
    let (base_best, _) = super::super::leaf::reach(s, p);
    let leader = (0..s.settings.players)
        .filter(|&o| o != p)
        .map(|o| road_length(s, o))
        .max()
        .unwrap_or(0)
        .max(4);
    let own_length = road_length(s, p);
    let contend = own.roads >= 4
        && own_length + 2 >= leader
        && s.longest_road != Some(p)
        && (w.road_contention_gate == 0.0 || super::super::leaf::road_production_lead(s, p));
    let deny = block_leader(s, p, w)
        .and_then(|q| super::super::bargain::contested_site(s, p, q).map(|v| (q, v)));
    let their_route = deny.map(|(q, _)| super::super::bargain::distances(s, q));
    let base_opp = if w.opponent_expansion != 0.0 {
        super::super::leaf::opponent_reach(s, p)
    } else {
        0.0
    };
    RoadBase {
        base_best,
        contend,
        own_length,
        deny,
        their_route,
        base_opp,
    }
}
/// Candidate score of one road: the gain in own reachable-site value and site count, a
/// bonus for extending a contested trail, the endgame credit for cutting the leader's
/// route to the contested site, and, with the `opponent_expansion` term on, the denied
/// value of the leading opponent's reachable sites. The trail bonus asks whether the seat
/// may contend for the award: four or more roads within two of the leader, and, with the
/// `road_contention_gate` weight on, the table's best brick and lumber production.
pub(super) fn road_score(s: &State, next: &State, p: u8, w: &Weights, base: &RoadBase) -> f64 {
    let (best, others) = super::super::leaf::reach(next, p);
    let mut score = best - base.base_best + f64::from(others) * 0.02;
    if base.contend && road_length(next, p) > base.own_length {
        score += 0.5;
    }
    if let (Some((q, v)), Some(route)) = (base.deny, &base.their_route) {
        let theirs = route[v as usize];
        let after = super::super::bargain::distances(next, q)[v as usize];
        let denied = f64::from(after.saturating_sub(theirs)).min(3.0);
        if theirs < u8::MAX && denied > 0.0 {
            score += w.endgame.block_bias * crate::leaf::site_value(s, v)
                / (1.0 + f64::from(theirs))
                * denied
                / 3.0
                * 0.5;
        }
    }
    if w.opponent_expansion != 0.0 {
        score +=
            w.opponent_expansion * (base.base_opp - super::super::leaf::opponent_reach(next, p));
    }
    score
}
pub fn road_preference(s: &State, p: u8, w: &Weights, cap: usize) -> Vec<Action> {
    let own = s.players[p as usize];
    if own.roads >= 15
        || !s.players[p as usize].resources.contains(Resources::ROAD) && s.phase == Phase::Main
    {
        return vec![];
    }
    let base = road_base(s, p, w);
    let mut scored: Vec<(f64, u8)> = Vec::new();
    for e in 0..72u8 {
        if !road_allowed(s, p, e) {
            continue;
        }
        let Ok(next) = apply(s, p, Action::BuildRoad { edge: e }) else {
            continue;
        };
        let score = road_score(s, &next, p, w, &base);
        if score > 1e-9 {
            scored.push((score, e));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(cap)
        .map(|(_, e)| Action::BuildRoad { edge: e })
        .collect()
}
/// Robber placements worth considering for seat `p` under the default scoring, with the
/// late endgame switch adding its leader bias when armed.
pub fn robber_preference(s: &State, p: u8, w: &Weights, cap: usize) -> Vec<Action> {
    robber_preference_with(s, p, w, cap, RobberConfig::default(), None, false)
}
/// The same shortlist under the robber targeting switches; see `RobberConfig`.
pub fn robber_preference_with(
    s: &State,
    p: u8,
    w: &Weights,
    cap: usize,
    robber: RobberConfig,
    expected_hands: Option<[[f64; 5]; 4]>,
    predict_threat: bool,
) -> Vec<Action> {
    let rows = robber_rows(s, p, w, robber, expected_hands, predict_threat);
    if rows.is_empty() {
        return vec![opponent::robber(s, p)];
    }
    let mut rows = rows;
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    rows.into_iter().take(cap).map(|(_, a)| a).collect()
}
/// Every legal robber placement with its static score, before the shortlist cut: the
/// production it blocks and the cards it steals, plus the endgame leader bias and the
/// robber targeting switches when armed. The score ranks the shortlist; it is not the
/// value the lookahead assigns.
pub fn robber_rows(
    s: &State,
    p: u8,
    w: &Weights,
    robber: RobberConfig,
    expected_hands: Option<[[f64; 5]; 4]>,
    predict_threat: bool,
) -> Vec<(f64, Action)> {
    let mut scored: Vec<(f64, Action)> = Vec::new();
    let endgame_leader = block_leader(s, p, w);
    let threats = robber
        .threat
        .then(|| super::super::bargain::threats(s, p, predict_threat));
    let target_leader = robber
        .leader
        .then(|| {
            (0..s.settings.players)
                .filter(|&o| o != p)
                .max_by_key(|&o| (public_points(s, o), std::cmp::Reverse(o)))
        })
        .flatten();
    let needed = if robber.need {
        needed_resource(s, p, w)
    } else {
        None
    };
    for hex in 0..19u8 {
        if hex == s.robber {
            continue;
        }
        let pips = f64::from(6u8.saturating_sub(s.layout.number(hex).abs_diff(7)));
        let mut blocked = 0.0;
        let mut leader_blocked = 0.0;
        let mut own = false;
        for &v in &topology().hex_vertices[hex as usize] {
            if let Some(o) = s.board.owner(v) {
                let scale = if s.board.building(v) > 4 { 2.0 } else { 1.0 };
                if o == p {
                    own = true;
                    continue;
                }
                let mut weight = 1.0 + f64::from(public_points(s, o)) * 0.15;
                if Some(o) == endgame_leader {
                    weight +=
                        w.endgame.block_bias * super::super::leaf::opponent_points(s, o, w) * 0.15;
                }
                let mut term = pips * scale * weight;
                if let Some(threats) = &threats {
                    term *= 1.0 + 2.0 * threats[o as usize].total;
                }
                blocked += term;
                if Some(o) == target_leader {
                    leader_blocked += term;
                }
            }
        }
        if own || s.layout.resource(hex).is_none() {
            continue;
        }
        let hex_score = if target_leader.is_some() {
            leader_blocked + 0.1 * (blocked - leader_blocked)
        } else {
            blocked
        };
        let victims = robber_victims(s, p, hex);
        if victims.is_empty() {
            scored.push((hex_score, Action::MoveRobber { hex, victim: None }));
        }
        for v in victims {
            let cards = f64::from(s.players[v as usize].resources.total()).min(8.0);
            let hand_term = match (needed, expected_hands) {
                (Some(r), Some(table)) => {
                    let expected = table[v as usize];
                    let total = expected.iter().sum::<f64>().max(1.0);
                    expected[r] / total * 2.8
                }
                _ => cards * 0.4,
            };
            let mut bonus = 1.0 + hand_term + f64::from(public_points(s, v)) * 0.3;
            if Some(v) == endgame_leader {
                bonus += w.endgame.block_bias * super::super::leaf::opponent_points(s, v, w) * 0.3;
            }
            scored.push((
                hex_score + bonus,
                Action::MoveRobber {
                    hex,
                    victim: Some(v),
                },
            ));
        }
    }
    // Never steal from a seat this seat wants to keep trading with while any placement
    // robbing a threatening seat exists.
    if let Some(threats) = &threats {
        let has_safe_victim = scored.iter().any(|(_, a)| {
            matches!(
                a,
                Action::MoveRobber {
                    victim: Some(v),
                    ..
                } if threats[*v as usize].total >= 0.5
            )
        });
        if has_safe_victim {
            scored.retain(|(_, a)| {
                !matches!(
                    a,
                    Action::MoveRobber {
                        victim: Some(v),
                        ..
                    } if threats[*v as usize].total < 0.5
                )
            });
        }
    }
    scored
}
/// The resource this seat most needs: the largest missing-card gap toward the builds it
/// is closest to affording, tie-broken by the leaf's resource values. None when the hand
/// already affords every build.
pub fn needed_resource(s: &State, p: u8, w: &Weights) -> Option<usize> {
    let own = s.players[p as usize].resources;
    let mut need = [0.0f64; 5];
    for cost in [
        Resources::CITY,
        Resources::SETTLEMENT,
        Resources::DEVELOPMENT,
        Resources::ROAD,
    ] {
        let weight = 1.0 / (1.0 + crate::leaf::deficit(own, cost));
        for (r, &missing) in cost.0.iter().enumerate() {
            need[r] += f64::from(missing.saturating_sub(own.0[r])) * weight;
        }
    }
    (0..5)
        .max_by(|&a, &b| {
            need[a]
                .total_cmp(&need[b])
                .then(w.resource_value[a].total_cmp(&w.resource_value[b]))
        })
        .filter(|&r| need[r] > 0.0)
}
