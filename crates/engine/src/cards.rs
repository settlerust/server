use crate::*;
pub(crate) fn buy(
    s: &mut State,
    p: u8,
    chance: Option<Chance>,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if s.phase != Phase::Main {
        return Err(RuleError::WrongPhase);
    }
    if s.deck_count() == 0 {
        return Err(RuleError::EmptyDeck);
    }
    crate::economy::pay(s, p, Resources::DEVELOPMENT)?;
    let drawn = match chance {
        Some(Chance::Development { card }) if s.deck[card as usize] > 0 => card,
        Some(_) => return Err(RuleError::InvalidChance),
        None if s.settings.deterministic => tape_draw(s),
        None => {
            let mut n = s.random.below(s.deck_count() as u32);
            let mut result = Development::Knight;
            for card in Development::ALL {
                if n < (s.deck[card as usize] as u32) {
                    result = card;
                    break;
                }
                n -= s.deck[card as usize] as u32;
            }
            result
        }
    };
    s.deck[drawn as usize] -= 1;
    s.players[p as usize].new_development[drawn as usize] += 1;
    emit(Event::BoughtDevelopment { player: p });
    Ok(())
}
/// The fixed order of the 25 development cards in a deterministic game.
pub fn deck_order(seed: [u8; 32]) -> [Development; 25] {
    let mut order = [Development::Knight; 25];
    let mut i = 0;
    for (card, count) in Development::ALL.iter().zip([14, 2, 2, 2, 5]) {
        for _ in 0..count {
            order[i] = *card;
            i += 1;
        }
    }
    Random::tape_shuffle(seed, Random::TAPE_DECK, &mut order);
    order
}
/// The next card of the fixed order that the deck still holds. Explicit search branches can
/// take other cards, so the tape skips cards that are no longer available.
fn tape_draw(s: &State) -> Development {
    let order = deck_order(s.random.seed);
    let drawn = 25 - s.deck_count() as usize;
    order[drawn..]
        .iter()
        .chain(order[..drawn].iter())
        .copied()
        .find(|&card| s.deck[card as usize] > 0)
        .unwrap_or(Development::Knight)
}
fn play(
    s: &mut State,
    p: u8,
    card: Development,
    emit: &mut impl FnMut(Event),
) -> Result<Resume, RuleError> {
    let resume = match s.phase {
        Phase::Roll => Resume::Roll,
        Phase::Main => Resume::Main,
        _ => return Err(RuleError::WrongPhase),
    };
    if s.played_development || s.players[p as usize].development[card as usize] == 0 {
        return Err(RuleError::CannotPlayDevelopment);
    }
    s.players[p as usize].development[card as usize] -= 1;
    s.used_development[card as usize] += 1;
    s.played_development = true;
    emit(Event::PlayedDevelopment { player: p, card });
    Ok(resume)
}
pub(crate) fn knight(s: &mut State, p: u8, emit: &mut impl FnMut(Event)) -> Result<(), RuleError> {
    let resume = play(s, p, Development::Knight, emit)?;
    s.players[p as usize].played_knights += 1;
    s.phase = Phase::Robber { resume };
    Ok(())
}
pub(crate) fn roads(s: &mut State, p: u8, emit: &mut impl FnMut(Event)) -> Result<(), RuleError> {
    if s.players[p as usize].roads == 15 || !(0..72).any(|e| crate::build::road_allowed(s, p, e)) {
        return Err(RuleError::NoPieces);
    }
    let resume = play(s, p, Development::RoadBuilding, emit)?;
    s.phase = Phase::FreeRoads {
        remaining: 2,
        resume,
    };
    Ok(())
}
pub(crate) fn finish_roads(s: &mut State) -> Result<(), RuleError> {
    let Phase::FreeRoads {
        remaining: 1,
        resume,
    } = s.phase
    else {
        return Err(RuleError::WrongPhase);
    };
    s.resume(resume);
    Ok(())
}
pub(crate) fn monopoly(
    s: &mut State,
    p: u8,
    resource: Resource,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    play(s, p, Development::Monopoly, emit)?;
    for other in 0..s.settings.players {
        if other == p {
            continue;
        }
        let count = s.players[other as usize].resources.0[resource as usize];
        s.players[other as usize].resources.0[resource as usize] = 0;
        s.players[p as usize].resources.0[resource as usize] += count;
        emit(Event::MonopolyCollected {
            player: p,
            victim: other,
            resource,
            count,
        });
    }
    Ok(())
}
pub(crate) fn plenty(
    s: &mut State,
    p: u8,
    resources: Resources,
    emit: &mut impl FnMut(Event),
) -> Result<(), RuleError> {
    if resources.total() != s.bank.total().min(2) {
        return Err(RuleError::BankShortage);
    }
    play(s, p, Development::YearOfPlenty, emit)?;
    crate::economy::take(s, p, resources)?;
    emit(Event::TookResources {
        player: p,
        resources,
    });
    Ok(())
}
