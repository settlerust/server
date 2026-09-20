use crate::*;
pub(crate) fn pay(s: &mut State, p: u8, resources: Resources) -> Result<(), RuleError> {
    if !s.players[p as usize].resources.contains(resources) {
        return Err(RuleError::InsufficientResources);
    }
    s.players[p as usize].resources.subtract(resources);
    s.bank.add(resources);
    Ok(())
}
pub(crate) fn take(s: &mut State, p: u8, resources: Resources) -> Result<(), RuleError> {
    if !s.bank.contains(resources) {
        return Err(RuleError::BankShortage);
    }
    s.bank.subtract(resources);
    s.players[p as usize].resources.add(resources);
    Ok(())
}
pub(crate) fn produce(s: &mut State, total: u8, emit: &mut impl FnMut(Event)) {
    let mut demand = [Resources::default(); 4];
    for h in 0..19 {
        if h == s.robber || s.layout.number(h) != total {
            continue;
        }
        if let Some(r) = s.layout.resource(h) {
            for &v in &topology().hex_vertices[h as usize] {
                if let Some(p) = s.board.owner(v)
                    && !s.is_retired(p)
                {
                    demand[p as usize].0[r as usize] += if s.board.building(v) > 4 { 2 } else { 1 };
                }
            }
        }
    }
    for r in 0..5 {
        let total: u8 = demand.iter().map(|d| d.0[r]).sum();
        if total > s.bank.0[r] {
            let recipients = demand.iter().filter(|d| d.0[r] > 0).count();
            for d in &mut demand {
                d.0[r] = if recipients == 1 && d.0[r] > 0 {
                    s.bank.0[r]
                } else {
                    0
                };
            }
        }
    }
    for (p, resources) in demand
        .iter()
        .copied()
        .enumerate()
        .take(s.settings.players as usize)
    {
        if resources.total() > 0 {
            s.bank.subtract(resources);
            s.players[p].resources.add(resources);
            emit(Event::Produced {
                player: p as u8,
                resources,
            });
        }
    }
}
pub fn trade_ratio(s: &State, p: u8, r: Resource) -> u8 {
    let mut ratio = 4;
    for (i, vertices) in topology().ports.iter().enumerate() {
        if vertices.iter().any(|&v| s.board.owner(v) == Some(p)) {
            if s.layout.ports[i] == r as u8 + 1 {
                return 2;
            }
            if s.layout.ports[i] == 0 {
                ratio = 3;
            }
        }
    }
    ratio
}
pub(crate) fn bank_trade(
    s: &mut State,
    p: u8,
    give: Resources,
    receive: Resources,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if s.phase != Phase::Main {
        return Err(RuleError::WrongPhase);
    }
    if give.total() == 0 || receive.total() == 0 || !give.disjoint(receive) {
        return Err(RuleError::InvalidTrade);
    }
    let mut credits = 0u16;
    for r in Resource::ALL {
        let ratio = trade_ratio(s, p, r);
        let count = give.0[r as usize];
        if !count.is_multiple_of(ratio) {
            return Err(RuleError::InvalidTrade);
        }
        credits += (count / ratio) as u16;
    }
    if credits != receive.total() {
        return Err(RuleError::InvalidTrade);
    }
    if !s.players[p as usize].resources.contains(give) {
        return Err(RuleError::InsufficientResources);
    }
    if !s.bank.contains(receive) {
        return Err(RuleError::BankShortage);
    }
    pay(s, p, give)?;
    take(s, p, receive)?;
    emit(Event::BankTraded {
        player: p,
        give,
        receive,
    });
    Ok(())
}
pub(crate) fn trade(
    s: &mut State,
    p: u8,
    other: u8,
    give: Resources,
    receive: Resources,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if s.phase != Phase::Main {
        return Err(RuleError::WrongPhase);
    }
    s.player(other)?;
    if s.is_retired(other)
        || other == p
        || (p != s.current && other != s.current)
        || give.total() == 0
        || receive.total() == 0
        || !give.disjoint(receive)
    {
        return Err(RuleError::InvalidTrade);
    }
    if !s.players[p as usize].resources.contains(give)
        || !s.players[other as usize].resources.contains(receive)
    {
        return Err(RuleError::InsufficientResources);
    }
    s.players[p as usize].resources.subtract(give);
    s.players[other as usize].resources.subtract(receive);
    s.players[p as usize].resources.add(receive);
    s.players[other as usize].resources.add(give);
    emit(Event::Traded {
        player: p,
        other,
        give,
        receive,
    });
    Ok(())
}
