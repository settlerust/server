//! Fixed additive baseline. Contributions are exact terms of this heuristic, not SHAP values.
use serde::{Deserialize, Serialize};
use settlers_engine::*;

/// Score terms in heuristic units; their sum is exactly the leaf value.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Features {
    pub points: f64,
    pub production: f64,
    pub diversity: f64,
    pub readiness: f64,
    pub expansion: f64,
    pub development: f64,
    pub opponent_progress: f64,
    pub terminal: f64,
}
impl Features {
    pub fn total(self) -> f64 {
        self.points
            + self.production
            + self.diversity
            + self.readiness
            + self.expansion
            + self.development
            + self.opponent_progress
            + self.terminal
    }
    pub fn add_scaled(&mut self, other: Self, weight: f64) {
        self.points += other.points * weight;
        self.production += other.production * weight;
        self.diversity += other.diversity * weight;
        self.readiness += other.readiness * weight;
        self.expansion += other.expansion * weight;
        self.development += other.development * weight;
        self.opponent_progress += other.opponent_progress * weight;
        self.terminal += other.terminal * weight;
    }
}
pub fn deficit(hand: Resources, cost: Resources) -> f64 {
    cost.0
        .iter()
        .zip(hand.0)
        .map(|(&need, held)| f64::from(need.saturating_sub(held)))
        .sum()
}
pub fn site_value(s: &State, v: u8) -> f64 {
    topology().incident_hexes[v as usize]
        .iter()
        .map(|&h| {
            let n = s.layout.number(h);
            if s.layout.resource(h).is_none() {
                0.0
            } else {
                f64::from(6 - n.abs_diff(7)) / 36.0
            }
        })
        .sum()
}
pub fn rates(s: &State, p: u8) -> [f64; 5] {
    let mut result = [0.0; 5];
    for h in 0..19 {
        if h == s.robber {
            continue;
        }
        let Some(r) = s.layout.resource(h) else {
            continue;
        };
        let rate = f64::from(6 - s.layout.number(h).abs_diff(7)) / 36.0;
        for &v in &topology().hex_vertices[h as usize] {
            if s.board.owner(v) == Some(p) {
                result[r as usize] += rate * if s.board.building(v) > 4 { 2.0 } else { 1.0 };
            }
        }
    }
    result
}
/// Best settlement site reachable from the current network; opponents block traversal.
pub fn expansion(s: &State, p: u8) -> (f64, Option<u8>) {
    let mut distance = [u8::MAX; 54];
    let mut first = [None; 54];
    let mut queue = Vec::new();
    for v in 0..54 {
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
                first[next as usize] = first[v as usize].or(Some(e));
                queue.push(next);
            }
        }
    }
    (0..54)
        .filter(|&v| {
            settlement_allowed(s, p, v, true)
                && distance[v as usize] <= 15 - s.players[p as usize].roads
        })
        .map(|v| {
            (
                site_value(s, v) / (1.0 + f64::from(distance[v as usize])),
                first[v as usize],
            )
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap_or((0.0, None))
}
/// Reads own cards and public board/opponent progress only, even in a sampled world.
pub fn evaluate(s: &State, p: u8) -> Features {
    let own = s.players[p as usize];
    let rates = rates(s, p);
    let costs = [
        Resources::CITY,
        Resources::SETTLEMENT,
        Resources::DEVELOPMENT,
    ];
    let readiness = costs
        .into_iter()
        .map(|cost| 12.0 / (1.0 + deficit(own.resources, cost)))
        .fold(0.0, f64::max);
    let terminal = match s.phase {
        Phase::Finished { winner } if winner == p => 10000.0,
        Phase::Finished { .. } => -10000.0,
        _ => 0.0,
    };
    Features {
        points: f64::from(points(s, p)) * 100.0,
        production: rates.iter().sum::<f64>() * 24.0,
        diversity: rates.iter().map(|r| r.min(0.15)).sum::<f64>() * 16.0,
        readiness,
        expansion: expansion(s, p).0 * 16.0,
        development: (0..4)
            .map(|r| f64::from(own.development[r] + own.new_development[r]) * 6.0)
            .sum(),
        opponent_progress: -35.0
            * (0..s.settings.players)
                .filter(|&other| other != p)
                .map(|other| f64::from(public_points(s, other)))
                .fold(0.0, f64::max),
        terminal,
    }
}

/// Cheap public/own-hand action ordering and deterministic continuation policy.
pub fn action_score(s: &State, p: u8, a: Action) -> f64 {
    let own = s.players[p as usize];
    let target = if own.cities < 4 && own.settlements > 0 {
        Resources::CITY
    } else {
        Resources::SETTLEMENT
    };
    match a {
        Action::BuildCity { vertex } => 100.0 + site_value(s, vertex) * 30.0,
        Action::BuildSettlement { vertex } => 100.0 + site_value(s, vertex) * 40.0,
        Action::BuildRoad { edge } => {
            let (_, preferred) = expansion(s, p);
            if preferred == Some(edge) {
                12.0
            } else if s.phase == Phase::Main {
                -1.0
            } else {
                1.0
            }
        }
        Action::RelocateSettlement { .. } => -100.0,
        Action::PlayKnight => 8.0,
        Action::PlayRoadBuilding => {
            if expansion(s, p).1.is_some() {
                10.0
            } else {
                -1.0
            }
        }
        Action::PlayYearOfPlenty { resources } => {
            8.0 + (0..5)
                .map(|r| {
                    f64::from(resources.0[r].min(target.0[r].saturating_sub(own.resources.0[r])))
                })
                .sum::<f64>()
        }
        Action::PlayMonopoly { resource } => {
            f64::from(
                target.0[resource as usize].saturating_sub(own.resources.0[resource as usize]),
            ) * 3.0
        }
        Action::BankTrade { give, receive } => {
            let after = Resources(std::array::from_fn(|r| {
                own.resources.0[r] - give.0[r] + receive.0[r]
            }));
            (deficit(own.resources, target) - deficit(after, target)) * 4.0 - 0.5
        }
        Action::BuyDevelopment => 3.0,
        Action::MoveRobber { hex, victim } => {
            let mut score = if victim.is_some() { 2.0 } else { 0.0 };
            for &v in &topology().hex_vertices[hex as usize] {
                if let Some(owner) = s.board.owner(v) {
                    score += if owner == p {
                        -10.0
                    } else {
                        f64::from(public_points(s, owner)) + 1.0
                    };
                }
            }
            score
        }
        Action::Roll => 1.0,
        _ => 0.0,
    }
}
