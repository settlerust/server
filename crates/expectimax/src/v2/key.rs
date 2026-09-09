//! Information-set key: everything `observe(state, Some(seat))` would show, hashed.
use settlers_engine::*;
use std::hash::{Hash, Hasher};

pub fn information_set(s: &State, seat: u8) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in 0..54u8 {
        s.board.building(v).hash(&mut h);
    }
    for e in 0..72u8 {
        s.board.road(e).hash(&mut h);
    }
    let own = s.players[seat as usize];
    own.resources.0.hash(&mut h);
    own.development.hash(&mut h);
    own.new_development.hash(&mut h);
    s.bank.0.hash(&mut h);
    s.deck_count().hash(&mut h);
    s.robber.hash(&mut h);
    s.current.hash(&mut h);
    s.turn.hash(&mut h);
    s.played_development.hash(&mut h);
    s.longest_road.hash(&mut h);
    s.largest_army.hash(&mut h);
    std::mem::discriminant(&s.phase).hash(&mut h);
    match s.phase {
        Phase::SetupSettlement { step } => step.hash(&mut h),
        Phase::SetupRoad { step, vertex } => (step, vertex).hash(&mut h),
        Phase::Discard { pending } => pending.hash(&mut h),
        Phase::Robber { resume } => (resume == Resume::Main).hash(&mut h),
        Phase::FreeRoads { remaining, resume } => (remaining, resume == Resume::Main).hash(&mut h),
        Phase::Finished { winner } => winner.hash(&mut h),
        Phase::Roll | Phase::Main => {}
    }
    for p in 0..s.settings.players {
        if p == seat {
            continue;
        }
        let other = s.players[p as usize];
        other.resources.total().hash(&mut h);
        (other.development.iter().sum::<u8>() + other.new_development.iter().sum::<u8>())
            .hash(&mut h);
        other.played_knights.hash(&mut h);
        (other.settlements, other.cities, other.roads).hash(&mut h);
    }
    h.finish()
}
