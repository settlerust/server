use crate::SearchError;
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;

pub fn random(seed: u32) -> Random {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&seed.to_le_bytes());
    bytes[4..8].copy_from_slice(b"exp1");
    Random::new(bytes)
}

pub fn worlds(
    o: &Observation,
    knowledge: &ResourceKnowledge,
    count: u8,
) -> Result<Vec<State>, SearchError> {
    o.settings.validate()?;
    let seat = o.seat.ok_or(SearchError::NotSeated)?;
    let own = o.own.ok_or(SearchError::NotSeated)?;
    if seat >= o.settings.players
        || o.current >= o.settings.players
        || o.robber >= 19
        || o.players.len() != o.settings.players as usize
        || o.players
            .iter()
            .enumerate()
            .any(|(i, p)| p.seat as usize != i || p.resource_count > 95)
        || o.bank.0.iter().any(|&n| n > 19)
        || o.bank.0.map(|n| n > 0) != o.bank_available
        || o.hexes.iter().any(|h| h & 15 > 5 || h >> 4 > 12)
        || o.ports.iter().any(|&p| p > 5)
    {
        return Err(SearchError::InvalidObservation);
    }
    let board = Board::from_public(&o.buildings, &o.roads)?;
    let first = match o.phase {
        Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => {
            if step >= 2 * o.settings.players {
                return Err(SearchError::InvalidObservation);
            }
            let offset = if step < o.settings.players {
                step
            } else {
                2 * o.settings.players - 1 - step
            };
            (o.current + o.settings.players - offset) % o.settings.players
        }
        _ => 0, // No rule consults `first` after setup.
    };
    let mut result = Vec::new();
    for sample in 0..count {
        let mut rng = random(u32::from(sample) + 1);
        let mut s = State {
            board,
            layout: Layout {
                hexes: o.hexes,
                ports: o.ports,
            },
            players: [Player::default(); 4],
            bank: Resources([19; 5]),
            deck: [14, 2, 2, 2, 5],
            used_development: knowledge.used_development,
            settings: o.settings,
            random: rng,
            phase: o.phase,
            current: o.current,
            first,
            robber: o.robber,
            played_development: o.played_development,
            longest_road: o.longest_road,
            largest_army: o.largest_army,
            turn: o.turn,
        };
        for p in &o.players {
            s.players[p.seat as usize] = Player {
                settlements: p.settlements,
                cities: p.cities,
                roads: p.roads,
                played_knights: p.played_knights,
                ..Player::default()
            };
        }
        s.players[seat as usize] = own;
        let hands = sample_hands(o, knowledge, &mut rng)?;
        for (p, hand) in hands.iter().enumerate().take(o.settings.players as usize) {
            s.players[p].resources = *hand;
        }
        for r in 0..5 {
            s.bank.0[r] = 19 - hands.iter().map(|h| h.0[r]).sum::<u8>();
        }
        // Known public knight plays stay known even after an incomplete reconnect history.
        s.used_development[0] = o.players.iter().map(|p| p.played_knights).sum();
        for r in 0..5 {
            s.deck[r] = s.deck[r]
                .checked_sub(own.development[r] + own.new_development[r] + s.used_development[r])
                .ok_or(SearchError::InconsistentBelief)?;
        }
        let held: u8 = o
            .players
            .iter()
            .filter(|p| p.seat != seat)
            .map(|p| p.development_count)
            .sum();
        let unknown_used = s
            .deck_count()
            .checked_sub(held + o.deck_count)
            .ok_or(SearchError::InconsistentBelief)?;
        for _ in 0..unknown_used {
            let r = draw(&s.deck[..4], &mut rng).ok_or(SearchError::InconsistentBelief)?;
            s.deck[r] -= 1;
            s.used_development[r] += 1;
        }
        for p in &o.players {
            if p.seat == seat {
                continue;
            }
            for _ in 0..p.development_count {
                let r = draw(&s.deck, &mut rng).ok_or(SearchError::InconsistentBelief)?;
                s.deck[r] -= 1;
                s.players[p.seat as usize].development[r] += 1;
            }
        }
        result.push(s);
    }
    Ok(result)
}
fn draw(counts: &[u8], rng: &mut Random) -> Option<usize> {
    let total: u32 = counts.iter().map(|&x| u32::from(x)).sum();
    if total == 0 {
        return None;
    }
    let mut n = rng.below(total);
    for (r, &count) in counts.iter().enumerate() {
        if n < u32::from(count) {
            return Some(r);
        }
        n -= u32::from(count);
    }
    None
}
fn sample_hands(
    o: &Observation,
    k: &ResourceKnowledge,
    rng: &mut Random,
) -> Result<[Resources; 4], SearchError> {
    // A small integral flow matches every player's total and every public resource
    // inventory. Reverse edges repair unlucky partial assignments instead of rejection.
    const SOURCE: usize = 9;
    const SINK: usize = 10;
    let count = o.settings.players as usize;
    let mut residual = [[0u16; 11]; 11];
    let mut capacity = [[0u16; 5]; 4];
    let mut hands = [Resources::default(); 4];
    for p in 0..count {
        hands[p] = Resources(k.players[p].minimum);
        residual[SOURCE][p] = k.players[p]
            .total
            .checked_sub(hands[p].total())
            .ok_or(SearchError::InconsistentBelief)?;
        for r in 0..5 {
            capacity[p][r] = u16::from(
                k.players[p].maximum[r]
                    .checked_sub(hands[p].0[r])
                    .ok_or(SearchError::InconsistentBelief)?,
            );
            residual[p][4 + r] = capacity[p][r];
        }
    }
    for r in 0..5 {
        let dealt: u16 = hands.iter().map(|hand| u16::from(hand.0[r])).sum();
        residual[4 + r][SINK] = 19u16
            .checked_sub(u16::from(o.bank.0[r]) + dealt)
            .ok_or(SearchError::InconsistentBelief)?;
    }
    if residual[SOURCE].iter().sum::<u16>() != (0..5).map(|r| residual[4 + r][SINK]).sum::<u16>() {
        return Err(SearchError::InconsistentBelief);
    }
    while residual[SOURCE].iter().any(|&n| n > 0) {
        let mut parent = [usize::MAX; 11];
        parent[SOURCE] = SOURCE;
        let mut queue = vec![SOURCE];
        let mut cursor = 0;
        while cursor < queue.len() && parent[SINK] == usize::MAX {
            let node = queue[cursor];
            cursor += 1;
            let mut neighbors: Vec<_> = (0..11)
                .filter(|&next| residual[node][next] > 0 && parent[next] == usize::MAX)
                .collect();
            while !neighbors.is_empty() {
                let weights: Vec<_> = neighbors
                    .iter()
                    .map(|&next| {
                        if node < count && (4..9).contains(&next) {
                            let r = next - 4;
                            let held = f64::from(hands[node].0[r])
                                + f64::from(capacity[node][r] - residual[node][next]);
                            ((k.players[node].expected[r] - held).max(0.1) * 100.0) as u32
                        } else {
                            1
                        }
                    })
                    .collect();
                let mut pick = rng.below(weights.iter().sum());
                let index = weights
                    .iter()
                    .position(|&weight| {
                        if pick < weight {
                            true
                        } else {
                            pick -= weight;
                            false
                        }
                    })
                    .unwrap_or(0);
                let next = neighbors.remove(index);
                parent[next] = node;
                queue.push(next);
            }
        }
        if parent[SINK] == usize::MAX {
            return Err(SearchError::InconsistentBelief);
        }
        let mut node = SINK;
        while node != SOURCE {
            let previous = parent[node];
            residual[previous][node] -= 1;
            residual[node][previous] += 1;
            node = previous;
        }
    }
    for p in 0..count {
        for r in 0..5 {
            hands[p].0[r] += (capacity[p][r] - residual[p][4 + r]) as u8;
        }
    }
    Ok(hands)
}

#[cfg(test)]
#[path = "../tests/internal/sampling.rs"]
mod sampling_tests;
