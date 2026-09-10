//! Information-set key: everything `observe(state, Some(seat))` would show, hashed.
//!
//! The key is a Zobrist-style combination: every visible field mixes its identity and value
//! through a 64-bit finalizer and the results are combined by exclusive or, in two independent
//! salted copies. Two positions that show the seat the same information share the key; the
//! second word verifies a hit, so a table collision cannot silently return another position's
//! value. Hidden hands, deck composition, and the random cursor never enter the key.
use settlers_engine::*;

/// Two independent 64-bit words; the first indexes a table, the second verifies the hit.
pub type Key = (u64, u64);

#[inline(always)]
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
const SALT_A: u64 = 0x9e37_79b9_7f4a_7c15;
const SALT_B: u64 = 0xd1b5_4a32_d192_ed03;

struct Mixer(u64, u64);
impl Mixer {
    #[inline(always)]
    fn field(&mut self, id: u64, value: u64) {
        let x = (id << 40) ^ value;
        self.0 ^= mix(x ^ SALT_A);
        self.1 ^= mix(x.rotate_left(17) ^ SALT_B);
    }
}

pub fn information_set(s: &State, seat: u8) -> Key {
    let mut m = Mixer(0, 0);
    for v in 0..54u8 {
        m.field(u64::from(v), u64::from(s.board.building(v)));
    }
    for e in 0..72u8 {
        m.field(
            100 + u64::from(e),
            s.board.road(e).map_or(0, |p| u64::from(p) + 1),
        );
    }
    let own = s.players[seat as usize];
    for r in 0..5 {
        m.field(200 + r as u64, u64::from(own.resources.0[r]));
        m.field(210 + r as u64, u64::from(own.development[r]));
        m.field(220 + r as u64, u64::from(own.new_development[r]));
        m.field(230 + r as u64, u64::from(s.bank.0[r]));
    }
    m.field(240, u64::from(s.deck_count()));
    m.field(241, u64::from(s.robber));
    m.field(242, u64::from(s.current));
    m.field(243, u64::from(s.turn));
    m.field(244, u64::from(s.played_development));
    m.field(245, s.longest_road.map_or(0, |p| u64::from(p) + 1));
    m.field(246, s.largest_army.map_or(0, |p| u64::from(p) + 1));
    let (kind, a, b) = match s.phase {
        Phase::SetupSettlement { step } => (1, u64::from(step), 0),
        Phase::SetupRoad { step, vertex } => (2, u64::from(step), u64::from(vertex)),
        Phase::Roll => (3, 0, 0),
        Phase::Discard { pending } => (4, u64::from(pending), 0),
        Phase::Robber { resume } => (5, u64::from(resume == Resume::Main), 0),
        Phase::FreeRoads { remaining, resume } => {
            (6, u64::from(remaining), u64::from(resume == Resume::Main))
        }
        Phase::Main => (7, 0, 0),
        Phase::Finished { winner } => (8, u64::from(winner), 0),
    };
    m.field(247, kind);
    m.field(248, a);
    m.field(249, b);
    for p in 0..s.settings.players {
        if p == seat {
            continue;
        }
        let other = s.players[p as usize];
        let base = 300 + u64::from(p) * 10;
        m.field(base, u64::from(other.resources.total()));
        m.field(
            base + 1,
            u64::from(
                other.development.iter().sum::<u8>() + other.new_development.iter().sum::<u8>(),
            ),
        );
        m.field(base + 2, u64::from(other.played_knights));
        m.field(base + 3, u64::from(other.settlements));
        m.field(base + 4, u64::from(other.cities));
        m.field(base + 5, u64::from(other.roads));
    }
    (m.0, m.1)
}
