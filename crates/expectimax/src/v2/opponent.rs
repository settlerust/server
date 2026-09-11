//! Deterministic builder model used for opponents and for greedy continuations. It is a port of
//! the server's fast builder that reads a seat's own sampled hand and the public board directly
//! from a world state, avoiding the allocation of a full observation and legal-action list.
use settlers_engine::*;

fn pips(number: u8) -> f64 {
    if (2..=12).contains(&number) && number != 7 {
        f64::from(6 - number.abs_diff(7))
    } else {
        0.0
    }
}
fn production(s: &State, vertex: u8) -> [f64; 5] {
    let mut values = [0.0; 5];
    for &h in &topology().incident_hexes[vertex as usize] {
        if let Some(r) = s.layout.resource(h) {
            values[r as usize] += pips(s.layout.number(h)) / 36.0;
        }
    }
    values
}
fn income(s: &State, p: u8) -> [f64; 5] {
    let mut total = [0.0; 5];
    for v in 0..54u8 {
        if s.board.owner(v) == Some(p) {
            let gain = production(s, v);
            let scale = if s.board.building(v) > 4 { 2.0 } else { 1.0 };
            for i in 0..5 {
                total[i] += gain[i] * scale;
            }
        }
    }
    total
}
fn vertex_score(s: &State, p: u8, v: u8) -> f64 {
    let gain = production(s, v);
    let current = income(s, p);
    gain.iter()
        .enumerate()
        .map(|(r, &x)| {
            x * (if current[r] < 0.01 { 2.2 } else { 1.0 }) * (if r >= 3 { 1.2 } else { 1.0 })
        })
        .sum()
}
fn possible_settlement(s: &State, v: u8) -> bool {
    s.board.building(v) == 0
        && topology().neighbors[v as usize]
            .iter()
            .all(|&n| s.board.building(n) == 0)
}
/// BFS from the seat's network to the best reachable site: (vertex, first edge, distance).
fn expansion(s: &State, p: u8) -> Option<(u8, Option<u8>, u8)> {
    let mut distance = [u8::MAX; 54];
    let mut first = [None; 54];
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
            if s.board.owner(next).is_some_and(|o| o != p) {
                continue;
            }
            let d = distance[v as usize] + 1;
            if d < distance[next as usize] {
                distance[next as usize] = d;
                first[next as usize] = first[v as usize].or(Some(e));
                queue.push(next);
            }
        }
    }
    let roads_left = 15 - s.players[p as usize].roads;
    (0..54u8)
        .filter(|&v| possible_settlement(s, v) && distance[v as usize] <= roads_left)
        .max_by(|&a, &b| {
            let score =
                |v: u8| vertex_score(s, p, v) / (1.0 + f64::from(distance[v as usize]) * 1.7);
            score(a).total_cmp(&score(b))
        })
        .map(|v| (v, first[v as usize], distance[v as usize]))
}
fn deficit(hand: Resources, cost: Resources) -> u16 {
    (0..5)
        .map(|i| u16::from(cost.0[i].saturating_sub(hand.0[i])))
        .sum()
}
/// The build the seat is saving toward and its cost.
pub fn target(s: &State, p: u8) -> (Option<Action>, Resources) {
    let own = s.players[p as usize];
    let mut options: Vec<(Action, Resources, f64, u8)> = vec![];
    if own.cities < 4 {
        for v in 0..54u8 {
            if s.board.building(v) == p + 1 {
                options.push((
                    Action::BuildCity { vertex: v },
                    Resources::CITY,
                    vertex_score(s, p, v) + 0.2,
                    0,
                ));
            }
        }
    }
    if own.settlements < 5
        && let Some((vertex, edge, d)) = expansion(s, p)
    {
        let (action, cost) = match edge {
            Some(edge) => (Action::BuildRoad { edge }, Resources::ROAD),
            None => (Action::BuildSettlement { vertex }, Resources::SETTLEMENT),
        };
        options.push((action, cost, vertex_score(s, p, vertex) + 0.25, d));
    }
    let score = |cost: Resources, gain: f64, d: u8| {
        (gain + 0.2) / (1.0 + f64::from(deficit(own.resources, cost)) * 0.15 + f64::from(d) * 0.6)
    };
    match options
        .into_iter()
        .max_by(|a, b| score(a.1, a.2, a.3).total_cmp(&score(b.1, b.2, b.3)))
    {
        Some((action, cost, _, _)) => (Some(action), cost),
        None => (Some(Action::BuyDevelopment), Resources::DEVELOPMENT),
    }
}
/// Largest-stack discard that protects the current target's cards.
pub fn discard(s: &State, p: u8) -> Resources {
    let own = s.players[p as usize];
    let (_, cost) = target(s, p);
    let mut hand = own.resources;
    let mut out = Resources::default();
    for _ in 0..hand.total() / 2 {
        let r = (0..5)
            .filter(|&r| hand.0[r] > 0)
            .max_by_key(|&r| (hand.0[r].saturating_sub(cost.0[r]), hand.0[r]))
            .unwrap_or(0);
        hand.0[r] -= 1;
        out.0[r] += 1;
    }
    out
}
/// Robber placement scored on blocked opponent pips and the victim's public score.
pub fn robber(s: &State, p: u8) -> Action {
    let mut best: Option<(f64, Action)> = None;
    for hex in 0..19u8 {
        if hex == s.robber {
            continue;
        }
        let victims = robber_victims(s, p, hex);
        let mut value = 0.0;
        for &v in &topology().hex_vertices[hex as usize] {
            if let Some(o) = s.board.owner(v) {
                value += pips(s.layout.number(hex)) * if o == p { -3.0 } else { 1.0 };
            }
        }
        let candidates: Vec<Action> = if victims.is_empty() {
            vec![Action::MoveRobber { hex, victim: None }]
        } else {
            victims
                .iter()
                .map(|&victim| Action::MoveRobber {
                    hex,
                    victim: Some(victim),
                })
                .collect()
        };
        for a in candidates {
            let bonus = match a {
                Action::MoveRobber {
                    victim: Some(v), ..
                } => f64::from(public_points(s, v)) + 1.0,
                _ => 0.0,
            };
            let score = value + bonus;
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, a));
            }
        }
    }
    best.map_or(
        Action::MoveRobber {
            hex: s.layout.desert(),
            victim: None,
        },
        |b| b.1,
    )
}
fn best_setup_settlement(s: &State, p: u8) -> Action {
    if let Some(action) = super::opening::place(s, p) {
        return action;
    }
    let vertex = (0..54u8)
        .filter(|&v| settlement_allowed(s, p, v, true))
        .max_by(|&a, &b| vertex_score(s, p, a).total_cmp(&vertex_score(s, p, b)))
        .unwrap_or(0);
    Action::BuildSettlement { vertex }
}
fn any_road(s: &State, p: u8) -> Option<Action> {
    if let Phase::SetupRoad { vertex, .. } = s.phase {
        return topology().incident_edges[vertex as usize]
            .iter()
            .find(|&&e| s.board.road(e).is_none())
            .map(|&e| Action::BuildRoad { edge: e });
    }
    (0..72u8)
        .find(|&e| road_allowed(s, p, e))
        .map(|e| Action::BuildRoad { edge: e })
}
/// The builder's action for seat `p` in world `s`. `steps` counts main-phase actions this turn.
pub fn act(s: &State, p: u8, steps: u32) -> Action {
    let own = s.players[p as usize];
    match s.phase {
        Phase::SetupSettlement { .. } => return best_setup_settlement(s, p),
        Phase::SetupRoad { vertex, .. } => {
            if let Some(action) = super::opening::place(s, p) {
                return action;
            }
            if let Some((_, Some(edge), _)) = expansion(s, p)
                && topology().edges[edge as usize].contains(&vertex)
                && s.board.road(edge).is_none()
            {
                return Action::BuildRoad { edge };
            }
            return any_road(s, p).unwrap_or(Action::FinishRoadBuilding);
        }
        Phase::FreeRoads { .. } => {
            if let Some((_, Some(edge), _)) = expansion(s, p)
                && road_allowed(s, p, edge)
            {
                return Action::BuildRoad { edge };
            }
            return any_road(s, p).unwrap_or(Action::FinishRoadBuilding);
        }
        Phase::Discard { .. } => {
            return Action::Discard {
                resources: discard(s, p),
            };
        }
        Phase::Robber { .. } => return robber(s, p),
        Phase::Finished { .. } => return Action::EndTurn,
        Phase::Roll | Phase::Main => {}
    }
    let can_play = !s.played_development;
    if can_play && own.development[Development::Knight as usize] > 0 && steps < 16 {
        return Action::PlayKnight;
    }
    if s.phase == Phase::Roll {
        return Action::Roll;
    }
    if steps >= 16 {
        return Action::EndTurn;
    }
    let (target, cost) = target(s, p);
    if let Some(action) = target
        && apply(s, p, action).is_ok()
    {
        return action;
    }
    // An immediately affordable point beats waiting for the planned target.
    if own.cities < 4 && own.resources.contains(Resources::CITY) {
        for v in 0..54u8 {
            if s.board.building(v) == p + 1 {
                return Action::BuildCity { vertex: v };
            }
        }
    }
    if own.settlements < 5 && own.resources.contains(Resources::SETTLEMENT) {
        for v in 0..54u8 {
            if settlement_allowed(s, p, v, false) {
                return Action::BuildSettlement { vertex: v };
            }
        }
    }
    if can_play
        && own.development[Development::RoadBuilding as usize] > 0
        && expansion(s, p).is_some()
        && apply(s, p, Action::PlayRoadBuilding).is_ok()
    {
        return Action::PlayRoadBuilding;
    }
    if can_play && own.development[Development::YearOfPlenty as usize] > 0 {
        let mut resources = Resources::default();
        let mut remaining = s.bank.total().min(2);
        for _ in 0..2 {
            if remaining == 0 {
                break;
            }
            let r = (0..5)
                .filter(|&r| s.bank.0[r] > resources.0[r])
                .max_by_key(|&r| cost.0[r].saturating_sub(own.resources.0[r] + resources.0[r]))
                .unwrap_or(0);
            resources.0[r] += 1;
            remaining -= 1;
        }
        let action = Action::PlayYearOfPlenty { resources };
        if apply(s, p, action).is_ok() {
            return action;
        }
    }
    if can_play && own.development[Development::Monopoly as usize] > 0 {
        let r = (0..5)
            .max_by_key(|&r| cost.0[r].saturating_sub(own.resources.0[r]))
            .unwrap_or(0);
        let action = Action::PlayMonopoly {
            resource: Resource::ALL[r],
        };
        if cost.0[r] > own.resources.0[r] && apply(s, p, action).is_ok() {
            return action;
        }
    }
    // Bank trade surplus toward the target when it reduces the deficit.
    let mut best: Option<(u16, Action)> = None;
    for give_r in 0..5 {
        let ratio = trade_ratio(s, p, Resource::ALL[give_r]);
        let surplus = own.resources.0[give_r].saturating_sub(cost.0[give_r]);
        if surplus < ratio {
            continue;
        }
        for get_r in 0..5 {
            if get_r == give_r || s.bank.0[get_r] == 0 || cost.0[get_r] <= own.resources.0[get_r] {
                continue;
            }
            let action = Action::BankTrade {
                give: Resources::one(Resource::ALL[give_r], ratio),
                receive: Resources::one(Resource::ALL[get_r], 1),
            };
            let key = u16::from(surplus);
            if best.as_ref().is_none_or(|b| key > b.0) {
                best = Some((key, action));
            }
        }
    }
    if let Some((_, action)) = best
        && apply(s, p, action).is_ok()
    {
        return action;
    }
    if s.deck_count() > 0
        && own.resources.contains(Resources::DEVELOPMENT)
        && (own.resources.total() > 7 || target == Some(Action::BuyDevelopment))
    {
        return Action::BuyDevelopment;
    }
    Action::EndTurn
}
