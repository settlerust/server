use crate::*;
pub fn settlement_allowed(s: &State, p: u8, v: u8, setup: bool) -> bool {
    v < 54
        && s.board.building(v) == 0
        && topology().neighbors[v as usize]
            .iter()
            .all(|&n| s.board.building(n) == 0)
        && (setup
            || topology().incident_edges[v as usize]
                .iter()
                .any(|&e| s.board.road(e) == Some(p)))
}
pub fn road_allowed(s: &State, p: u8, e: u8) -> bool {
    e < 72
        && s.board.road(e).is_none()
        && topology().edges[e as usize]
            .iter()
            .any(|&v| match s.board.owner(v) {
                Some(owner) => owner == p,
                None => topology().incident_edges[v as usize]
                    .iter()
                    .any(|&adj| s.board.road(adj) == Some(p)),
            })
}
pub(crate) fn build_settlement(
    s: &mut State,
    p: u8,
    v: u8,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    let step = match s.phase {
        Phase::SetupSettlement { step } => Some(step),
        Phase::Main => None,
        _ => return Err(RuleError::WrongPhase),
    };
    if !settlement_allowed(s, p, v, step.is_some()) {
        return Err(RuleError::IllegalPlacement);
    }
    if s.players[p as usize].settlements >= 5 {
        return Err(RuleError::NoPieces);
    }
    if step.is_none() {
        crate::economy::pay(s, p, Resources::SETTLEMENT)?;
    }
    s.board.set_building(v, p + 1);
    s.players[p as usize].settlements += 1;
    emit(Event::BuiltSettlement {
        player: p,
        vertex: v,
    });
    if let Some(step) = step {
        if step >= s.settings.players {
            let mut resources = Resources::default();
            for &h in &topology().incident_hexes[v as usize] {
                if let Some(r) = s.layout.resource(h) {
                    resources.0[r as usize] += 1;
                }
            }
            crate::economy::take(s, p, resources)?;
            emit(Event::Produced {
                player: p,
                resources,
            });
        }
        s.phase = Phase::SetupRoad { step, vertex: v };
    }
    Ok(())
}
pub(crate) fn build_road(
    s: &mut State,
    p: u8,
    e: u8,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if e >= 72 || s.board.road(e).is_some() {
        return Err(RuleError::IllegalPlacement);
    }
    match s.phase {
        Phase::SetupRoad { vertex, .. } if topology().edges[e as usize].contains(&vertex) => {}
        Phase::Main | Phase::FreeRoads { .. } if road_allowed(s, p, e) => {}
        Phase::Main | Phase::FreeRoads { .. } | Phase::SetupRoad { .. } => {
            return Err(RuleError::IllegalPlacement);
        }
        _ => return Err(RuleError::WrongPhase),
    }
    if s.players[p as usize].roads >= 15 {
        return Err(RuleError::NoPieces);
    }
    if s.phase == Phase::Main {
        crate::economy::pay(s, p, Resources::ROAD)?;
    }
    s.board.set_road(e, p);
    s.players[p as usize].roads += 1;
    emit(Event::BuiltRoad { player: p, edge: e });
    match s.phase {
        Phase::SetupRoad { step, .. } => {
            let step = step + 1;
            if step == 2 * s.settings.players {
                s.current = s.first;
                s.phase = Phase::Roll;
                s.turn = 1;
                emit(Event::TurnChanged {
                    player: s.first,
                    turn: 1,
                });
            } else {
                s.current = s.setup_player(step);
                s.phase = Phase::SetupSettlement { step };
            }
        }
        Phase::FreeRoads { remaining, resume } => {
            if remaining == 1
                || s.players[p as usize].roads == 15
                || !(0..72).any(|edge| road_allowed(s, p, edge))
            {
                s.resume(resume);
            } else {
                s.phase = Phase::FreeRoads {
                    remaining: remaining - 1,
                    resume,
                };
            }
        }
        _ => {}
    }
    Ok(())
}
pub(crate) fn relocate_settlement(
    s: &mut State,
    p: u8,
    vertex: u8,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    let Phase::SetupRoad { step, vertex: from } = s.phase else {
        return Err(RuleError::WrongPhase);
    };
    if vertex == from || s.board.building(from) != p + 1 {
        return Err(RuleError::IllegalPlacement);
    }
    let mut returned = Resources::default();
    if step >= s.settings.players {
        for &h in &topology().incident_hexes[from as usize] {
            if let Some(resource) = s.layout.resource(h) {
                returned.0[resource as usize] += 1;
            }
        }
        crate::economy::pay(s, p, returned)?;
    }
    s.board.set_building(from, 0);
    s.players[p as usize].settlements -= 1;
    s.phase = Phase::SetupSettlement { step };
    let mut received = Resources::default();
    build_settlement(s, p, vertex, &mut |event| {
        if let Event::Produced { resources, .. } = event {
            received = resources;
        }
    })?;
    emit(Event::SetupSettlementMoved {
        player: p,
        from,
        vertex,
        returned,
        received,
    });
    Ok(())
}
pub(crate) fn build_city(
    s: &mut State,
    p: u8,
    v: u8,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if s.phase != Phase::Main {
        return Err(RuleError::WrongPhase);
    }
    if v >= 54 || s.board.building(v) != p + 1 {
        return Err(RuleError::IllegalPlacement);
    }
    if s.players[p as usize].cities >= 4 {
        return Err(RuleError::NoPieces);
    }
    crate::economy::pay(s, p, Resources::CITY)?;
    s.board.set_building(v, p + 5);
    s.players[p as usize].settlements -= 1;
    s.players[p as usize].cities += 1;
    emit(Event::BuiltCity {
        player: p,
        vertex: v,
    });
    Ok(())
}
