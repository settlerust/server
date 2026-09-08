#![allow(dead_code)]
use settlers_engine::*;
pub fn game() -> State {
    let mut state = State::new(Settings::default(), [7; 32]).unwrap();
    state.first = 0;
    state.current = 0;
    state
}
pub fn started() -> State {
    let mut s = game();
    while s.turn == 0 {
        let a = legal_actions(&s, s.current)[0];
        s = apply(&s, s.current, a).unwrap();
    }
    s
}
pub fn main_phase() -> State {
    let mut s = started();
    s.phase = Phase::Main;
    s
}
pub fn hand(s: &mut State, p: u8, r: Resources) {
    s.bank
        .0
        .iter_mut()
        .zip(s.players[p as usize].resources.0)
        .for_each(|(a, b)| *a += b);
    s.players[p as usize].resources = r;
    s.bank.0.iter_mut().zip(r.0).for_each(|(a, b)| *a -= b);
}
pub fn dev(s: &mut State, p: u8, d: Development) {
    s.deck[d as usize] -= 1;
    s.players[p as usize].development[d as usize] += 1;
}
pub fn board(buildings: &[(u8, u8)], roads: &[(u8, u8)]) -> Board {
    let mut vs = [0u8; 27];
    let mut es = [0u8; 27];
    for &(v, value) in buildings {
        vs[v as usize / 2] |= value << ((v % 2) * 4);
    }
    for &(e, p) in roads {
        let b = e as usize * 3;
        for i in 0..3 {
            es[(b + i) / 8] |= (((p + 1) >> i) & 1) << ((b + i) % 8);
        }
    }
    serde_json::from_value(serde_json::json!({"vertices":vs,"roads":es})).unwrap()
}
pub fn conservation(s: &State) {
    for r in 0..5 {
        assert_eq!(
            s.bank.0[r] as u16
                + s.players
                    .iter()
                    .map(|p| p.resources.0[r] as u16)
                    .sum::<u16>(),
            19
        );
    }
    for (d, initial) in [14, 2, 2, 2, 5].iter().enumerate() {
        assert_eq!(
            s.deck[d] as u16
                + s.used_development[d] as u16
                + s.players
                    .iter()
                    .map(|p| (p.development[d] + p.new_development[d]) as u16)
                    .sum::<u16>(),
            *initial
        );
    }
    for p in 0..s.settings.players {
        let player = s.players[p as usize];
        assert_eq!(
            (0..54).filter(|&v| s.board.building(v) == p + 1).count(),
            player.settlements as usize
        );
        assert_eq!(
            (0..54).filter(|&v| s.board.building(v) == p + 5).count(),
            player.cities as usize
        );
        assert_eq!(
            (0..72).filter(|&e| s.board.road(e) == Some(p)).count(),
            player.roads as usize
        );
        assert!(player.settlements <= 5 && player.cities <= 4 && player.roads <= 15);
    }
}
