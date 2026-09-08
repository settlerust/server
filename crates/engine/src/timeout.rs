use crate::*;
/// Resolve one expired turn (or one setup settlement+road pair) deterministically.
/// Forced robber movement goes to the desert without stealing, even if already there.
pub fn timeout(input: &State) -> Result<(State, Vec<Event>), RuleError> {
    if matches!(input.phase, Phase::Finished { .. }) {
        return Err(RuleError::GameFinished);
    }
    let mut state = *input;
    let mut events = vec![Event::TimedOut {
        player: state.current,
    }];
    let original_turn = state.turn;
    let setup = matches!(
        state.phase,
        Phase::SetupSettlement { .. } | Phase::SetupRoad { .. }
    );
    for _ in 0..16 {
        if matches!(state.phase, Phase::Finished { .. }) {
            break;
        }
        let actor = state.current;
        if let Phase::Robber { resume } = state.phase {
            state.robber = state.layout.desert();
            events.push(Event::RobberMoved {
                player: actor,
                hex: state.robber,
            });
            state.resume(resume);
            events.push(Event::PhaseChanged { phase: state.phase });
            continue;
        }
        let (actor, action) = match state.phase {
            Phase::Discard { pending } => {
                let p = pending.trailing_zeros() as u8;
                (
                    p,
                    Action::Discard {
                        resources: automatic_discard(state.players[p as usize].resources),
                    },
                )
            }
            Phase::Roll => (actor, Action::Roll),
            Phase::Main => (actor, Action::EndTurn),
            _ => (
                actor,
                *legal_actions(&state, actor)
                    .first()
                    .ok_or(RuleError::IllegalPlacement)?,
            ),
        };
        let old_phase = state.phase;
        let (next, more) = apply_recorded(&state, actor, action)?;
        state = next;
        events.extend(more);
        if (setup && matches!(old_phase, Phase::SetupRoad { .. }))
            || (!setup && state.turn != original_turn)
        {
            break;
        }
    }
    Ok((state, events))
}
