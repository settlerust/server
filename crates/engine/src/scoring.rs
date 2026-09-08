use crate::*;
/// Maximum edge-simple trail. Opponent intersections may be endpoints but never transit nodes.
pub fn road_length(s: &State, p: u8) -> u8 {
    fn walk(s: &State, p: u8, v: u8, used: u128, entered: bool) -> u8 {
        if entered && s.board.owner(v).is_some_and(|owner| owner != p) {
            return 0;
        }
        topology().incident_edges[v as usize]
            .iter()
            .filter(|&&e| s.board.road(e) == Some(p) && used & (1u128 << e) == 0)
            .map(|&e| {
                let [a, b] = topology().edges[e as usize];
                let next = if a == v { b } else { a };
                1 + walk(s, p, next, used | (1u128 << e), true)
            })
            .max()
            .unwrap_or(0)
    }
    (0..54).map(|v| walk(s, p, v, 0, false)).max().unwrap_or(0)
}
fn award(values: [u8; 4], players: u8, minimum: u8, holder: Option<u8>) -> Option<u8> {
    let maximum = *values[..players as usize].iter().max().unwrap_or(&0);
    if maximum < minimum {
        return None;
    }
    if holder.is_some_and(|p| values[p as usize] == maximum) {
        return holder;
    }
    let mut contenders = (0..players).filter(|&p| values[p as usize] == maximum);
    let first = contenders.next();
    if contenders.next().is_none() {
        first
    } else {
        None
    }
}
pub fn public_points(s: &State, p: u8) -> u8 {
    let player = &s.players[p as usize];
    player.settlements
        + 2 * player.cities
        + 2 * u8::from(s.longest_road == Some(p))
        + 2 * u8::from(s.largest_army == Some(p))
}
pub fn points(s: &State, p: u8) -> u8 {
    public_points(s, p)
        + s.players[p as usize].development[4]
        + s.players[p as usize].new_development[4]
}
pub(crate) fn update_awards(s: &mut State, roads_changed: bool, emit: &mut impl FnMut(Event)) {
    let old = (s.longest_road, s.largest_army);
    if roads_changed {
        s.longest_road = award(
            std::array::from_fn(|p| road_length(s, p as u8)),
            s.settings.players,
            5,
            s.longest_road,
        );
    }
    s.largest_army = award(
        s.players.map(|p| p.played_knights),
        s.settings.players,
        3,
        s.largest_army,
    );
    if old != (s.longest_road, s.largest_army) {
        emit(Event::AwardsChanged {
            longest_road: s.longest_road,
            largest_army: s.largest_army,
        });
    }
}
pub(crate) fn check_win(s: &mut State, emit: &mut impl FnMut(Event)) {
    if s.turn > 0
        && !matches!(s.phase, Phase::Finished { .. })
        && points(s, s.current) >= s.settings.points_to_win
    {
        let p = s.current;
        s.phase = Phase::Finished { winner: p };
        emit(Event::Won {
            player: p,
            points: points(s, p),
        });
    }
}
