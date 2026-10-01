use crate::*;

/// Validate and apply one action without allocating or mutating the input.
///
/// The seed and random cursor are part of the state, so the same inputs produce
/// the same successor. Invalid actions return a rule error without a successor.
///
/// ```
/// use settlers_engine::{Action, Settings, State, apply, legal_actions};
///
/// let initial = State::new(Settings::default(), [7; 32])?;
/// let action = legal_actions(&initial, initial.current)[0];
/// let next = apply(&initial, initial.current, action)?;
/// assert_ne!(next.board, initial.board);
/// assert_eq!(next, apply(&initial, initial.current, action)?);
/// # Ok::<(), settlers_engine::RuleError>(())
/// ```
pub fn apply(state: &State, player: u8, action: Action) -> Result<State, RuleError> {
    execute(state, player, action, None, &mut |_| {})
}
/// An explicit chance branch for solvers and rule fixtures. Never expose on the network.
pub fn apply_with_chance(
    state: &State,
    player: u8,
    action: Action,
    chance: Chance,
) -> Result<State, RuleError> {
    execute(state, player, action, Some(chance), &mut |_| {})
}
/// The server adapter: return public effects only after the whole action succeeds.
pub fn apply_recorded(
    state: &State,
    player: u8,
    action: Action,
) -> Result<(State, Vec<Event>), RuleError> {
    let mut events = Vec::new();
    let state = execute(state, player, action, None, &mut |event| events.push(event))?;
    Ok((state, events))
}
fn execute(
    input: &State,
    p: u8,
    action: Action,
    chance: Option<Chance>,
    emit: &mut impl FnMut(Event),
) -> Result<State, RuleError> {
    input.player(p)?;
    if input.is_retired(p) {
        return Err(RuleError::InvalidPlayer);
    }
    if matches!(input.phase, Phase::Finished { .. }) {
        return Err(RuleError::GameFinished);
    }
    if p != input.current && !matches!(action, Action::Discard { .. } | Action::Trade { .. }) {
        return Err(RuleError::NotYourTurn);
    }
    if chance.is_some()
        && !matches!(
            action,
            Action::Roll | Action::MoveRobber { .. } | Action::BuyDevelopment
        )
    {
        return Err(RuleError::InvalidChance);
    }
    let mut s = *input;
    match action {
        Action::RelocateSettlement { vertex } => {
            crate::build::relocate_settlement(&mut s, p, vertex, emit)?
        }
        Action::BuildSettlement { vertex } => {
            crate::build::build_settlement(&mut s, p, vertex, emit)?
        }
        Action::BuildRoad { edge } => crate::build::build_road(&mut s, p, edge, emit)?,
        Action::BuildCity { vertex } => crate::build::build_city(&mut s, p, vertex, emit)?,
        Action::Roll => roll(&mut s, p, chance, emit)?,
        Action::Discard { resources } => discard(&mut s, p, resources, emit)?,
        Action::MoveRobber { hex, victim } => robber(&mut s, p, hex, victim, chance, emit)?,
        Action::BuyDevelopment => crate::cards::buy(&mut s, p, chance, emit)?,
        Action::PlayKnight => crate::cards::knight(&mut s, p, emit)?,
        Action::PlayRoadBuilding => crate::cards::roads(&mut s, p, emit)?,
        Action::FinishRoadBuilding => crate::cards::finish_roads(&mut s)?,
        Action::PlayMonopoly { resource } => crate::cards::monopoly(&mut s, p, resource, emit)?,
        Action::PlayYearOfPlenty { resources } => crate::cards::plenty(&mut s, p, resources, emit)?,
        Action::BankTrade { give, receive } => {
            crate::economy::bank_trade(&mut s, p, give, receive, emit)?
        }
        Action::Trade {
            other,
            give,
            receive,
        } => crate::economy::trade(&mut s, p, other, give, receive, emit)?,
        Action::EndTurn => end_turn(&mut s, emit)?,
    }
    crate::retirement::skip_setup(&mut s, emit);
    crate::scoring::update_awards(
        &mut s,
        matches!(
            action,
            Action::BuildRoad { .. } | Action::BuildSettlement { .. }
        ),
        emit,
    );
    crate::scoring::check_win(&mut s, emit);
    if s.phase != input.phase {
        emit(Event::PhaseChanged { phase: s.phase });
    }
    Ok(s)
}
fn roll(
    s: &mut State,
    p: u8,
    chance: Option<Chance>,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if s.phase != Phase::Roll {
        return Err(RuleError::WrongPhase);
    }
    let (first, second) = match chance {
        Some(Chance::Dice { first, second })
            if (1..=6).contains(&first) && (1..=6).contains(&second) =>
        {
            (first, second)
        }
        Some(_) => return Err(RuleError::InvalidChance),
        None if s.settings.deterministic => s.random.tape_roll(),
        None => (s.random.below(6) as u8 + 1, s.random.below(6) as u8 + 1),
    };
    emit(Event::Rolled {
        player: p,
        first,
        second,
    });
    if first + second == 7 {
        let pending = (0..s.settings.players).fold(0, |mask, p| {
            mask | if s.players[p as usize].resources.total() > s.settings.discard_limit as u16 {
                1 << p
            } else {
                0
            }
        });
        s.phase = if pending == 0 {
            Phase::Robber {
                resume: Resume::Main,
            }
        } else {
            Phase::Discard { pending }
        };
    } else {
        crate::economy::produce(s, first + second, emit);
        s.phase = Phase::Main;
    }
    Ok(())
}
fn discard(
    s: &mut State,
    p: u8,
    resources: Resources,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    let Phase::Discard { pending } = s.phase else {
        return Err(RuleError::WrongPhase);
    };
    if pending & (1 << p) == 0 || resources.total() != s.players[p as usize].resources.total() / 2 {
        return Err(RuleError::InvalidDiscard);
    }
    crate::economy::pay(s, p, resources)?;
    let pending = pending & !(1 << p);
    s.phase = if pending == 0 {
        Phase::Robber {
            resume: Resume::Main,
        }
    } else {
        Phase::Discard { pending }
    };
    emit(Event::Discarded {
        player: p,
        count: resources.total(),
    });
    Ok(())
}
/// Eligible victims have at least one card; resource types remain hidden.
pub fn robber_victims(s: &State, p: u8, hex: u8) -> Vec<u8> {
    let mask = victim_mask(s, p, hex);
    (0..s.settings.players)
        .filter(|&p| mask & (1 << p) != 0)
        .collect()
}
fn victim_mask(s: &State, p: u8, hex: u8) -> u8 {
    if hex >= 19 {
        return 0;
    }
    (0..s.settings.players)
        .filter(|&other| {
            other != p
                && s.players[other as usize].resources.total() > 0
                && topology().hex_vertices[hex as usize]
                    .iter()
                    .any(|&v| s.board.owner(v) == Some(other))
        })
        .fold(0, |mask, p| mask | (1 << p))
}
fn robber(
    s: &mut State,
    p: u8,
    hex: u8,
    victim: Option<u8>,
    chance: Option<Chance>,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    let Phase::Robber { resume } = s.phase else {
        return Err(RuleError::WrongPhase);
    };
    if hex >= 19 || hex == s.robber {
        return Err(RuleError::InvalidLocation);
    }
    let victims = victim_mask(s, p, hex);
    if (victims != 0 && victim.is_none())
        || victim.is_some_and(|v| v >= s.settings.players || victims & (1 << v) == 0)
    {
        return Err(RuleError::InvalidVictim);
    }
    if victim.is_none() && chance.is_some() {
        return Err(RuleError::InvalidChance);
    }
    s.robber = hex;
    emit(Event::RobberMoved { player: p, hex });
    if let Some(victim) = victim {
        let hand = s.players[victim as usize].resources;
        let resource = match chance {
            Some(Chance::Steal { resource }) if hand.0[resource as usize] > 0 => resource,
            Some(_) => return Err(RuleError::InvalidChance),
            None => {
                let mut n = if s.settings.deterministic {
                    s.random.tape_steal(hand.total() as u32)
                } else {
                    s.random.below(hand.total() as u32)
                };
                let mut chosen = Resource::Brick;
                for r in Resource::ALL {
                    if n < hand.0[r as usize] as u32 {
                        chosen = r;
                        break;
                    }
                    n -= hand.0[r as usize] as u32;
                }
                chosen
            }
        };
        s.players[victim as usize].resources.0[resource as usize] -= 1;
        s.players[p as usize].resources.0[resource as usize] += 1;
        emit(Event::Stole {
            player: p,
            victim,
            count: 1,
        });
    }
    s.resume(resume);
    Ok(())
}
fn end_turn(s: &mut State, emit: &mut impl FnMut(Event)) -> Result<(), RuleError> {
    if s.phase != Phase::Main {
        return Err(RuleError::WrongPhase);
    }
    let player = &mut s.players[s.current as usize];
    for i in 0..5 {
        player.development[i] += player.new_development[i];
        player.new_development[i] = 0;
    }
    s.current = s.next_player(s.current);
    s.turn += 1;
    s.played_development = false;
    s.phase = Phase::Roll;
    emit(Event::TurnChanged {
        player: s.current,
        turn: s.turn,
    });
    Ok(())
}
