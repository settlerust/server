use crate::*;

/// Remove a seat from play while preserving every occupied edge and vertex.
/// Authority and elapsed-time checks belong to the server, outside the pure engine.
pub fn retire(input: &State, seat: u8) -> Result<(State, Vec<Event>), RuleError> {
    input.player(seat)?;
    if input.is_retired(seat) {
        return Err(RuleError::InvalidPlayer);
    }
    if matches!(input.phase, Phase::Finished { .. }) {
        return Err(RuleError::GameFinished);
    }
    let mut s = *input;
    let mut events = vec![];
    s.retired |= 1 << seat;
    let player = &mut s.players[seat as usize];
    s.bank.add(player.resources);
    player.resources = Resources::default();
    for i in 0..5 {
        s.used_development[i] += player.development[i] + player.new_development[i];
        player.development[i] = 0;
        player.new_development[i] = 0;
    }
    player.played_knights = 0;
    let mut remaining = (0..s.settings.players).filter(|&p| !s.is_retired(p));
    let first = remaining.next();
    let last_player = remaining.next().is_none();
    if last_player {
        let winner = first.ok_or(RuleError::InvalidPlayer)?;
        s.current = winner;
        s.phase = Phase::Finished { winner };
        events.push(Event::Won {
            player: winner,
            points: points(&s, winner),
        });
    } else if let Phase::Discard { pending } = s.phase {
        let pending = pending & !(1 << seat);
        s.phase = if pending == 0 {
            Phase::Robber {
                resume: Resume::Main,
            }
        } else {
            Phase::Discard { pending }
        };
        if s.current == seat {
            s.current = s.next_player(seat);
        }
    } else if matches!(
        s.phase,
        Phase::SetupSettlement { .. } | Phase::SetupRoad { .. }
    ) {
        skip_setup(&mut s, &mut |event| events.push(event));
    } else if s.current == seat {
        s.current = s.next_player(seat);
        s.turn += 1;
        s.played_development = false;
        s.phase = Phase::Roll;
        events.push(Event::TurnChanged {
            player: s.current,
            turn: s.turn,
        });
    }
    crate::scoring::update_awards(&mut s, true, &mut |event| events.push(event));
    if s.phase != input.phase {
        events.push(Event::PhaseChanged { phase: s.phase });
    }
    Ok((s, events))
}

pub(crate) fn skip_setup(s: &mut State, emit: &mut impl FnMut(Event)) {
    while s.is_retired(s.current) {
        let step = match s.phase {
            Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => step + 1,
            Phase::Roll => {
                s.current = s.next_player(s.current);
                emit(Event::TurnChanged {
                    player: s.current,
                    turn: s.turn,
                });
                return;
            }
            _ => return,
        };
        if step == 2 * s.settings.players {
            s.current = if s.is_retired(s.first) {
                s.next_player(s.first)
            } else {
                s.first
            };
            s.phase = Phase::Roll;
            s.turn = 1;
            emit(Event::TurnChanged {
                player: s.current,
                turn: s.turn,
            });
            return;
        }
        s.current = s.setup_player(step);
        s.phase = Phase::SetupSettlement { step };
    }
}
