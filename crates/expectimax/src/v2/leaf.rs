//! Additive leaf for the v2 search. Reads own cards and public information in a sampled world.
use super::weights::Weights;
use crate::leaf::site_value;
use serde::Serialize;
use settlers_engine::*;
use std::collections::BTreeMap;

/// Named terms whose sum is exactly the leaf value.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Terms(pub BTreeMap<&'static str, f64>);
impl Terms {
    pub fn total(&self) -> f64 {
        self.0.values().sum()
    }
    pub fn add_scaled(&mut self, other: &Terms, weight: f64) {
        for (k, v) in &other.0 {
            *self.0.entry(k).or_insert(0.0) += v * weight;
        }
    }
    pub fn scale(&mut self, factor: f64) {
        for v in self.0.values_mut() {
            *v *= factor;
        }
    }
    pub fn owned(&self) -> BTreeMap<String, f64> {
        self.0.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }
}

/// Expected cards per roll for a seat, per resource, excluding the robber's hex.
pub fn rates(s: &State, p: u8) -> [f64; 5] {
    crate::leaf::rates(s, p)
}
/// Reachable legal settlement sites: best value/(1+distance) and the count of others.
pub fn reach(s: &State, p: u8) -> (f64, u32) {
    let (value, others, distance) = reach_detail(s, p);
    (value / (1.0 + f64::from(distance)), others)
}
/// Whether the seat ties or leads the table in brick plus lumber production per roll.
/// The `road_contention_gate` weight restricts the longest-road contention credit to
/// seats that hold this lead: they are the ones who can keep a road race going.
pub fn road_production_lead(s: &State, p: u8) -> bool {
    let own = {
        let r = rates(s, p);
        r[0] + r[1]
    };
    (0..s.settings.players).filter(|&q| q != p).all(|q| {
        let r = rates(s, q);
        r[0] + r[1] <= own + 1e-12
    })
}
/// The leading opponent's best reachable site value, discounted by its road distance.
/// This is the expansion the `opponent_expansion` term denies: a road on the opponent's
/// shortest path raises their distance and lowers this quantity.
pub fn opponent_reach(s: &State, p: u8) -> f64 {
    let mut best = 0.0f64;
    for q in 0..s.settings.players {
        if q == p || s.players[q as usize].settlements >= 5 {
            continue;
        }
        let (value, _, distance) = reach_detail(s, q);
        best = best.max(value / (1.0 + f64::from(distance)));
    }
    best
}
/// Turns until a cost is affordable from the hand at the seat's production per own turn.
pub fn eta_turns(s: &State, hand: Resources, cost: Resources, rates: &[f64; 5]) -> f64 {
    let per_turn = f64::from(s.settings.players);
    (0..5)
        .map(|r| f64::from(cost.0[r].saturating_sub(hand.0[r])) / (rates[r] * per_turn + 0.03))
        .fold(0.0, f64::max)
}
/// The best target build, valued by its gain discounted by the turns needed to afford it.
fn plan_value(
    s: &State,
    p: u8,
    rates: &[f64; 5],
    w: &Weights,
    future: f64,
    best_site: f64,
    best_distance: u8,
) -> f64 {
    let own = s.players[p as usize];
    let mut best = 0.0f64;
    let mut consider = |gain: f64, cost: Resources| {
        let eta = eta_turns(s, own.resources, cost, rates);
        best = best.max(gain / (1.0 + eta));
    };
    if own.cities < 4 {
        let mut top = 0.0f64;
        for v in 0..54u8 {
            if s.board.building(v) == p + 1 {
                top = top.max(site_value(s, v));
            }
        }
        if top > 0.0 {
            consider(w.point + top * future * 0.9, Resources::CITY);
        }
    }
    if own.settlements < 5 && best_site > 0.0 {
        let mut cost = Resources::SETTLEMENT;
        cost.0[0] = cost.0[0].saturating_add(best_distance);
        cost.0[1] = cost.0[1].saturating_add(best_distance);
        consider(w.point + best_site * future * 0.9, cost);
    }
    if s.deck_count() > 0 {
        consider(0.2 * w.point + w.development, Resources::DEVELOPMENT);
    }
    best * w.readiness
}
/// Best reachable site value, count of other sites, and the best site's road distance.
pub fn reach_detail(s: &State, p: u8) -> (f64, u32, u8) {
    let mut distance = [u8::MAX; 54];
    let mut queue = Vec::with_capacity(54);
    for v in 0..54u8 {
        if s.board.owner(v) == Some(p)
            || (s.board.building(v) == 0
                && topology().incident_edges[v as usize]
                    .iter()
                    .any(|&e| s.board.road(e) == Some(p)))
        {
            distance[v as usize] = 0;
            queue.push(v);
        }
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let v = queue[cursor];
        cursor += 1;
        for &e in &topology().incident_edges[v as usize] {
            if s.board.road(e).is_some() {
                continue;
            }
            let [a, b] = topology().edges[e as usize];
            let next = if a == v { b } else { a };
            if s.board.owner(next).is_some_and(|owner| owner != p) {
                continue;
            }
            if distance[next as usize] > distance[v as usize] + 1 {
                distance[next as usize] = distance[v as usize] + 1;
                queue.push(next);
            }
        }
    }
    let roads_left = 15 - s.players[p as usize].roads;
    let mut best = (0.0f64, 0u8);
    let mut count = 0u32;
    for v in 0..54u8 {
        if distance[v as usize] <= roads_left && settlement_allowed(s, p, v, true) {
            count += 1;
            let value = site_value(s, v) / (1.0 + f64::from(distance[v as usize]));
            if value > best.0 / (1.0 + f64::from(best.1)) {
                best = (site_value(s, v), distance[v as usize]);
            }
        }
    }
    (best.0, count.saturating_sub(1), best.1)
}
fn port_value(s: &State, p: u8, rates: &[f64; 5], w: &Weights, future: f64) -> f64 {
    let mut value = 0.0f64;
    let mut generic = false;
    for (i, vertices) in topology().ports.iter().enumerate() {
        if !vertices.iter().any(|&v| s.board.owner(v) == Some(p)) {
            continue;
        }
        match s.layout.ports[i] {
            0 => generic = true,
            r => value += w.port_match * rates[(r - 1) as usize] * future,
        }
    }
    if generic {
        value += w.port_generic;
    }
    value
}
/// Rolls that remain in the game under the configured length estimate.
pub fn remaining_rolls(s: &State, w: &Weights) -> f64 {
    (w.turns_per_player * f64::from(s.settings.players) - f64::from(s.turn)).max(w.minimum_rolls)
}
/// The points an opponent is credited with: public points, plus the hidden victory cards
/// sampled into this world when the endgame switch is on. Averaged over worlds this is the
/// expected hidden score under the remaining deck composition.
pub fn opponent_points(s: &State, q: u8, w: &Weights) -> f64 {
    if w.endgame.hidden_points {
        f64::from(points(s, q))
    } else {
        f64::from(public_points(s, q))
    }
}
/// The opponent with the most points, first seat winning ties.
pub fn leader_seat(s: &State, p: u8, w: &Weights) -> Option<u8> {
    (0..s.settings.players)
        .filter(|&q| q != p)
        .max_by(|&a, &b| opponent_points(s, a, w).total_cmp(&opponent_points(s, b, w)))
}
/// Expected new victory points seat `q` gains on their next turn: the best of a city, a
/// reachable settlement, or a development purchase, times the share of its cost their hand
/// plus one roll of their own production covers. The remaining deck prices the purchase's
/// hidden victory points, and blocked production counts for less.
pub fn expected_next_gain(s: &State, q: u8) -> f64 {
    let own = s.players[q as usize];
    let hand = own.resources;
    let rates = rates(s, q);
    let cover = |cost: Resources| -> f64 {
        let need = f64::from(cost.total());
        if need <= 0.0 {
            return 1.0;
        }
        let have: f64 = (0..5)
            .map(|r| {
                let held = f64::from(hand.0[r]).min(f64::from(cost.0[r]));
                held + rates[r].min(f64::from(cost.0[r]) - held)
            })
            .sum();
        (have / need).min(1.0)
    };
    let mut best = 0.0f64;
    if own.cities < 4 && (0..54u8).any(|v| s.board.building(v) == q + 1) {
        best = best.max(cover(Resources::CITY));
    }
    if own.settlements < 5 {
        let (site, _, distance) = reach_detail(s, q);
        if site > 0.0 {
            let mut cost = Resources::SETTLEMENT;
            cost.0[0] = cost.0[0].saturating_add(distance);
            cost.0[1] = cost.0[1].saturating_add(distance);
            best = best.max(cover(cost));
        }
    }
    if s.deck_count() > 0 {
        let vps = f64::from(s.deck[4]) / f64::from(s.deck_count());
        best = best.max(cover(Resources::DEVELOPMENT) * vps);
    }
    best
}
/// Knights the seat still needs to take the largest army: at least three, and strictly more
/// than the best opponent. Zero when the seat already holds the award.
pub fn army_need(s: &State, p: u8) -> u8 {
    if s.largest_army == Some(p) {
        return 0;
    }
    let best_other = (0..s.settings.players)
        .filter(|&o| o != p)
        .map(|o| s.players[o as usize].played_knights)
        .max()
        .unwrap_or(0);
    (best_other.max(2) + 1).saturating_sub(s.players[p as usize].played_knights)
}
/// The `knight_play_value` schedule: played knights are worth more the closer the seat is to
/// taking the largest army (two away, one away, taking it) rather than the flat weight.
fn army_tier(need: u8) -> f64 {
    match need {
        0 => 1.25, // holds the award: the knights spent on it defend the two points
        1 => 1.75, // one knight away
        2 => 1.0,  // two away
        _ => 0.5,  // three or more away
    }
}
/// Per-roll production of the seat's buildings on the robber's hex: what a knight play
/// would unblock. Zero when the robber blocks nothing of this seat's.
pub fn robbed_production(s: &State, p: u8, w: &Weights) -> f64 {
    if s.layout.resource(s.robber).is_none() {
        return 0.0;
    }
    let pips = f64::from(6u8.saturating_sub(s.layout.number(s.robber).abs_diff(7)));
    if pips == 0.0 {
        return 0.0;
    }
    let mut rate = 0.0;
    for &v in &topology().hex_vertices[s.robber as usize] {
        if s.board.owner(v) == Some(p) {
            rate += pips / 36.0 * if s.board.building(v) > 4 { 2.0 } else { 1.0 };
        }
    }
    let resource = s
        .layout
        .resource(s.robber)
        .map(|r| w.resource_value[r as usize]);
    rate * resource.unwrap_or(0.0)
}
pub fn evaluate(s: &State, p: u8, w: &Weights) -> Terms {
    evaluate_with(s, p, w, false)
}
/// The hand-written leaf. With `knight_play_value` the development term values played
/// knights by largest-army distance and credits a held playable knight with one roll of the
/// production it would unblock; without it the flat weights apply.
pub fn evaluate_with(s: &State, p: u8, w: &Weights, knight_play_value: bool) -> Terms {
    let own = s.players[p as usize];
    let rates = rates(s, p);
    let mut t = BTreeMap::new();
    let finished = matches!(s.phase, Phase::Finished { .. });
    if let Phase::Finished { winner } = s.phase {
        t.insert(
            "terminal",
            if winner == p { w.terminal } else { -w.terminal },
        );
    }
    let own_points = f64::from(points(s, p));
    t.insert("points", own_points * w.point);
    let mut max_opp = 0.0f64;
    let mut leader = None;
    let mut sum_opp = 0.0;
    let mut max_opp_rate = 0.0f64;
    for other in 0..s.settings.players {
        if other == p {
            continue;
        }
        let pts = opponent_points(s, other, w);
        if leader.is_none() || pts > max_opp {
            max_opp = pts;
            leader = Some(other);
        }
        sum_opp += pts;
        if w.opponent_production != 0.0 {
            max_opp_rate = max_opp_rate.max(crate::leaf::rates(s, other).iter().sum::<f64>());
        }
    }
    // Race leaf: with an opponent close to victory, the plain opponent terms give way to
    // the own-minus-leader margin and the leader's expected points next turn.
    let to_win = f64::from(s.settings.points_to_win.max(3));
    if w.endgame.race && max_opp >= to_win - f64::from(w.endgame.trigger) {
        t.insert(
            "race_margin",
            (own_points - max_opp) * w.point * w.endgame.margin,
        );
        let tempo = leader.map_or(0.0, |q| expected_next_gain(s, q));
        t.insert("leader_tempo", -tempo * w.point * w.endgame.tempo);
    } else {
        t.insert(
            "opponent_progress",
            max_opp * w.opponent_max_point + sum_opp * w.opponent_sum_point,
        );
        if w.opponent_production != 0.0 {
            t.insert("opponent_production", max_opp_rate * w.opponent_production);
        }
    }
    if finished {
        return Terms(t);
    }
    let rolls = remaining_rolls(s, w);
    let future = w.card_value * rolls;
    let rate_value: f64 = rates.iter().zip(w.resource_value).map(|(r, v)| r * v).sum();
    t.insert("production", rate_value * future);
    t.insert(
        "hand",
        f64::from(own.resources.total()) * w.card_value * w.hand_discount,
    );
    t.insert(
        "diversity",
        rates.iter().map(|r| (r / 0.08).min(1.0)).sum::<f64>() / 5.0 * w.diversity,
    );
    let (best_site, others, best_distance) = if own.settlements < 5 {
        reach_detail(s, p)
    } else {
        (0.0, 0, 0)
    };
    t.insert(
        "plan",
        plan_value(s, p, &rates, w, future, best_site, best_distance),
    );
    if own.settlements < 5 {
        t.insert(
            "expansion",
            best_site / (1.0 + f64::from(best_distance)) * future * w.expansion
                + f64::from(others.min(6)) * w.expansion_sites,
        );
    }
    if own.settlements < 5 && w.denial != 0.0 {
        // A site another seat's predicted line is about to take, which this seat could still
        // take or block first, is worth less than the expansion term credits: the value is
        // discounted by how soon the opponent can pay for it. Taking or blocking the site
        // removes the discount from every later leaf.
        let ours = super::bargain::distances(s, p);
        let mut denial = 0.0f64;
        for q in 0..s.settings.players {
            if q == p {
                continue;
            }
            if let Some(build) = super::predict::next_build(s, q, s.players[q as usize].resources)
                && matches!(
                    build.action,
                    Action::BuildSettlement { .. } | Action::BuildRoad { .. }
                )
                && let Some(v) = build.site
                && settlement_allowed(s, p, v, true)
                && ours[v as usize] <= 3
            {
                denial +=
                    crate::leaf::site_value(s, v) * future * w.denial / (1.0 + build.site_eta);
            }
        }
        t.insert("denial", -denial);
    }
    let held: f64 = (0..4)
        .map(|r| f64::from(own.development[r] + own.new_development[r]))
        .sum();
    let knights = if knight_play_value {
        let played = f64::from(own.played_knights) * w.knight * army_tier(army_need(s, p));
        let unblock = if own.development[Development::Knight as usize] > 0 {
            robbed_production(s, p, w) * w.card_value
        } else {
            0.0
        };
        played + unblock
    } else {
        f64::from(own.played_knights) * w.knight
    };
    t.insert("development", held * w.development + knights);
    let length = if own.roads >= 4 {
        road_length(s, p)
    } else {
        own.roads
    };
    let mut road = f64::from(length) * w.road_length;
    if s.longest_road != Some(p)
        && own.roads >= 4
        && own.roads < 15
        && (w.road_contention_gate == 0.0 || road_production_lead(s, p))
    {
        let leader = (0..s.settings.players)
            .filter(|&o| o != p)
            .map(|o| road_length(s, o))
            .max()
            .unwrap_or(0)
            .max(4);
        if length + 1 > leader {
            road += w.road_contention * 0.5;
        }
    }
    t.insert("road", road);
    if w.opponent_expansion != 0.0 {
        t.insert(
            "opponent_expansion",
            -opponent_reach(s, p) * future * w.opponent_expansion,
        );
    }
    let cards = own.resources.total();
    if cards > s.settings.discard_limit as u16 {
        let seven = 1.0 - (5.0f64 / 6.0).powi(i32::from(s.settings.players));
        let loss = seven * f64::from(cards / 2) * w.card_value * w.hand_discount;
        t.insert("hand_risk", -loss * w.hand_risk);
    }
    t.insert("ports", port_value(s, p, &rates, w, future));
    Terms(t)
}
