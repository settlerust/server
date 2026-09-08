use settlers_engine::*;
fn owner(value: u8) -> Option<u8> {
    value.checked_sub(1).map(|v| v % 4)
}
fn pips(number: u8) -> f64 {
    if (2..=12).contains(&number) && number != 7 {
        (6 - (7i16 - number as i16).abs()) as f64
    } else {
        0.0
    }
}
fn production(o: &Observation, vertex: u8) -> [f64; 5] {
    let mut values = [0.0; 5];
    for &h in &topology().incident_hexes[vertex as usize] {
        let b = o.hexes[h as usize];
        if b & 15 != 0 {
            values[((b & 15) - 1) as usize] += pips(b >> 4) / 36.0;
        }
    }
    values
}
fn income(o: &Observation) -> [f64; 5] {
    let mut total = [0.0; 5];
    for v in 0..54 {
        if owner(o.buildings[v]) == o.seat {
            let gain = production(o, v as u8);
            for i in 0..5 {
                total[i] += gain[i] * if o.buildings[v] > 4 { 2.0 } else { 1.0 };
            }
        }
    }
    total
}
fn vertex_score(o: &Observation, v: u8) -> f64 {
    let gain = production(o, v);
    let current = income(o);
    gain.iter()
        .enumerate()
        .map(|(r, &x)| {
            x * (if current[r] < 0.01 { 2.2 } else { 1.0 }) * (if r >= 3 { 1.2 } else { 1.0 })
        })
        .sum()
}
fn possible_settlement(o: &Observation, v: u8) -> bool {
    o.buildings[v as usize] == 0
        && topology().neighbors[v as usize]
            .iter()
            .all(|&n| o.buildings[n as usize] == 0)
}
/// BFS from our road network to a legal future settlement. An occupied enemy node blocks transit.
fn expansion(o: &Observation) -> Option<(u8, Option<u8>, u8)> {
    let seat = o.seat?;
    let mut distance = [u8::MAX; 54];
    let mut first = [None; 54];
    let mut queue = std::collections::VecDeque::new();
    for (v, distance_to_vertex) in distance.iter_mut().enumerate() {
        if owner(o.buildings[v]) == Some(seat)
            || (o.buildings[v] == 0
                && topology().incident_edges[v]
                    .iter()
                    .any(|&e| o.roads[e as usize] == Some(seat)))
        {
            *distance_to_vertex = 0;
            queue.push_back(v as u8);
        }
    }
    while let Some(v) = queue.pop_front() {
        for &e in &topology().incident_edges[v as usize] {
            if o.roads[e as usize].is_some() {
                continue;
            }
            let [a, b] = topology().edges[e as usize];
            let next = if a == v { b } else { a };
            if owner(o.buildings[next as usize]).is_some_and(|p| p != seat) {
                continue;
            }
            let d = distance[v as usize] + 1;
            if d < distance[next as usize] {
                distance[next as usize] = d;
                first[next as usize] = first[v as usize].or(Some(e));
                queue.push_back(next);
            }
        }
    }
    (0..54)
        .filter(|&v| {
            possible_settlement(o, v) && distance[v as usize] <= 15 - o.own.map_or(15, |p| p.roads)
        })
        .max_by(|&a, &b| {
            let score = |v: u8| vertex_score(o, v) / (1.0 + distance[v as usize] as f64 * 1.7);
            score(a).total_cmp(&score(b))
        })
        .map(|v| (v, first[v as usize], distance[v as usize]))
}
pub(crate) fn deficit(hand: Resources, cost: Resources) -> u16 {
    (0..5)
        .map(|i| cost.0[i].saturating_sub(hand.0[i]) as u16)
        .sum()
}
pub(crate) fn target(o: &Observation, analytical: bool) -> (Option<Action>, Resources) {
    let Some(own) = o.own else {
        return (None, Resources::DEVELOPMENT);
    };
    let seat = o.seat.unwrap_or(0);
    let mut options = vec![];
    if own.cities < 4 {
        for v in 0..54 {
            if o.buildings[v] == seat + 1 {
                options.push((
                    Action::BuildCity { vertex: v as u8 },
                    Resources::CITY,
                    vertex_score(o, v as u8) + 0.2,
                    0,
                ));
            }
        }
    }
    if own.settlements < 5
        && let Some((vertex, edge, d)) = expansion(o)
    {
        let (action, cost) = if let Some(edge) = edge {
            (Action::BuildRoad { edge }, Resources::ROAD)
        } else {
            (Action::BuildSettlement { vertex }, Resources::SETTLEMENT)
        };
        options.push((action, cost, vertex_score(o, vertex) + 0.25, d));
    }
    let rates = income(o);
    let score = |cost: Resources, gain: f64, d: u8| {
        if analytical {
            let eta = (0..5)
                .map(|r| cost.0[r].saturating_sub(own.resources.0[r]) as f64 / (rates[r] + 0.025))
                .fold(0.0, f64::max);
            (gain + 0.2) / (1.0 + eta / 8.0 + d as f64 * 0.5)
        } else {
            (gain + 0.2) / (1.0 + deficit(own.resources, cost) as f64 * 0.15 + d as f64 * 0.6)
        }
    };
    if let Some((action, cost, _, _)) = options
        .into_iter()
        .max_by(|a, b| score(a.1, a.2, a.3).total_cmp(&score(b.1, b.2, b.3)))
    {
        (Some(action), cost)
    } else {
        (Some(Action::BuyDevelopment), Resources::DEVELOPMENT)
    }
}
pub(crate) fn choose(o: &Observation, analytical: bool) -> Option<Action> {
    let own = o.own?;
    let seat = o.seat?;
    let legal = &o.legal_actions;
    if legal.is_empty() {
        return None;
    }
    if matches!(o.phase, Phase::SetupSettlement { .. }) {
        return legal.iter().copied().max_by(|a, b| {
            let score = |a: Action| {
                if let Action::BuildSettlement { vertex } = a {
                    vertex_score(o, vertex)
                } else {
                    0.0
                }
            };
            score(*a).total_cmp(&score(*b))
        });
    }
    if matches!(o.phase, Phase::SetupRoad { .. } | Phase::FreeRoads { .. }) {
        if let Some((_, Some(edge), _)) = expansion(o)
            && legal.contains(&Action::BuildRoad { edge })
        {
            return Some(Action::BuildRoad { edge });
        }
        return legal.first().copied();
    }
    let (target, cost) = target(o, analytical);
    if matches!(o.phase, Phase::Discard { .. }) {
        let mut hand = own.resources;
        let mut resources = Resources::default();
        for _ in 0..hand.total() / 2 {
            let r = (0..5)
                .filter(|&r| hand.0[r] > 0)
                .max_by_key(|&r| (hand.0[r].saturating_sub(cost.0[r]), hand.0[r]))
                .unwrap_or(0);
            hand.0[r] -= 1;
            resources.0[r] += 1;
        }
        return Some(Action::Discard { resources });
    }
    if matches!(o.phase, Phase::Robber { .. }) {
        return legal.iter().copied().max_by(|a, b| {
            let score = |a: Action| {
                if let Action::MoveRobber { hex, victim } = a {
                    let mut value =
                        victim.map_or(0.0, |p| o.players[p as usize].points as f64 + 1.0);
                    for &v in &topology().hex_vertices[hex as usize] {
                        if let Some(p) = owner(o.buildings[v as usize]) {
                            value += pips(o.hexes[hex as usize] >> 4)
                                * if p == seat { -3.0 } else { 1.0 };
                        }
                    }
                    value
                } else {
                    0.0
                }
            };
            score(*a).total_cmp(&score(*b))
        });
    }
    if legal.contains(&Action::PlayKnight) {
        return Some(Action::PlayKnight);
    }
    if o.phase == Phase::Roll {
        return Some(Action::Roll);
    }
    if let Some(action) = target
        && legal.contains(&action)
    {
        return Some(action);
    }
    // Build an immediately affordable point before spending a turn waiting for another target.
    if let Some(action) = legal
        .iter()
        .copied()
        .find(|a| matches!(a, Action::BuildCity { .. } | Action::BuildSettlement { .. }))
    {
        return Some(action);
    }
    if legal.contains(&Action::PlayRoadBuilding) && expansion(o).is_some() {
        return Some(Action::PlayRoadBuilding);
    }
    if let Some(action) = legal
        .iter()
        .copied()
        .filter(|a| matches!(a, Action::PlayYearOfPlenty { .. }))
        .max_by_key(|a| {
            let Action::PlayYearOfPlenty { resources } = a else {
                return 0;
            };
            (0..5)
                .map(|i| resources.0[i].min(cost.0[i].saturating_sub(own.resources.0[i])) as u16)
                .sum::<u16>()
        })
    {
        return Some(action);
    }
    if let Some(resource) = (0..5)
        .max_by_key(|&r| cost.0[r].saturating_sub(own.resources.0[r]))
        .map(|r| Resource::ALL[r])
        && legal.contains(&Action::PlayMonopoly { resource })
    {
        return Some(Action::PlayMonopoly { resource });
    }
    if let Some(action) = legal
        .iter()
        .copied()
        .filter(|a| matches!(a, Action::BankTrade { .. }))
        .filter(|a| {
            let Action::BankTrade { give, receive } = a else {
                return false;
            };
            let mut after = own.resources;
            for i in 0..5 {
                after.0[i] = after.0[i] - give.0[i] + receive.0[i];
            }
            deficit(after, cost) < deficit(own.resources, cost)
        })
        .max_by_key(|a| {
            let Action::BankTrade { give, .. } = a else {
                return 0;
            };
            (0..5)
                .map(|i| give.0[i].min(own.resources.0[i].saturating_sub(cost.0[i])) as u16)
                .sum::<u16>()
        })
    {
        return Some(action);
    }
    if legal.contains(&Action::BuyDevelopment)
        && (own.resources.total() > 7 || target == Some(Action::BuyDevelopment))
    {
        return Some(Action::BuyDevelopment);
    }
    Some(Action::EndTurn)
}
