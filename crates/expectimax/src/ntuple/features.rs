//! Pattern extraction for the n-tuple leaf. Every pattern is read from one seat's information
//! in a world: the public board and layout, the seat's own hand and development cards, and the
//! public counts of the other seats. No hidden opponent hand, seed, cursor, phase, or turn
//! ownership enters a pattern, so one table serves every seat and both search and training.
use settlers_engine::*;

/// Corner states around a hex and at a vertex, from the seat's perspective.
const EMPTY: usize = 0;
const OWN_SETTLEMENT: usize = 1;
const OWN_CITY: usize = 2;
const OTHER_SETTLEMENT: usize = 3;
const OTHER_CITY: usize = 4;
const CORNER_STATES: usize = 5;

/// Six corners in five states, times resource (six with the desert), pips (0 to 5) and robber.
pub const HEX_PATTERNS: usize = 15_625 * 6 * 6 * 2;
/// Three touching hexes (36 resource-pip codes, plus the sea), times six building states,
/// four road-adjacency states, and seven port kinds.
pub const VERTEX_PATTERNS: usize = 37 * 37 * 37 * 6 * 4 * 7;
/// The compact intersection window: the three touching hexes and the building state only.
pub const CORNER_PATTERNS: usize = 37 * 37 * 37 * 6;
/// Production pips per resource, each capped at 15.
pub const PRODUCTION_PATTERNS: usize = 16 * 16 * 16 * 16 * 16;
/// Hand per resource capped at 4, hand total capped at 8, settlements and cities left.
pub const HAND_PATTERNS: usize = 3_125 * 9 * 6 * 5;
/// Own points, the two best opponent scores, and the turn bucket.
pub const RACE_PATTERNS: usize = 13 * 13 * 13 * 8;
/// Knights, cards held, army and road holders, own trail, roads left, and the deck bucket.
pub const DEVELOPMENT_PATTERNS: usize = 4 * 4 * 3 * 3 * 16 * 16 * 4;
/// Largest opponent hand, opponent production, opponent count, own production, best opponent.
pub const OPPONENT_PATTERNS: usize = 10 * 32 * 3 * 32 * 13;

pub const HEX_COUNT: usize = 19;
pub const VERTEX_COUNT: usize = 54;
pub const CORNER_COUNT: usize = 54;
pub const PORTFOLIO_COUNT: usize = 5;
/// Upper bound on active entries per position.
pub const MAX_ACTIVE: usize = HEX_COUNT + VERTEX_COUNT + CORNER_COUNT + PORTFOLIO_COUNT;

fn pips(number: u8) -> usize {
    if (2..=12).contains(&number) && number != 7 {
        usize::from(6 - number.abs_diff(7))
    } else {
        0
    }
}
fn corner(s: &State, seat: u8, v: u8) -> usize {
    match s.board.owner(v) {
        None => EMPTY,
        Some(o) if o == seat => {
            if s.board.building(v) > 4 {
                OWN_CITY
            } else {
                OWN_SETTLEMENT
            }
        }
        Some(_) => {
            if s.board.building(v) > 4 {
                OTHER_CITY
            } else {
                OTHER_SETTLEMENT
            }
        }
    }
}
/// The smallest base-5 code of the six corners over the hex's twelve symmetries.
fn canonical_corners(c: &[usize; 6]) -> usize {
    let mut best = usize::MAX;
    for start in 0..6 {
        let mut forward = 0usize;
        let mut backward = 0usize;
        for i in 0..6 {
            forward = forward * CORNER_STATES + c[(start + i) % 6];
            backward = backward * CORNER_STATES + c[(start + 6 - i) % 6];
        }
        best = best.min(forward).min(backward);
    }
    best
}
/// Resource-pip code of a hex: 0 for the desert, `resource * 6 + pips` otherwise.
fn hex_code(s: &State, h: u8) -> usize {
    match s.layout.resource(h) {
        None => 0,
        Some(r) => (r as usize + 1) * 6 + pips(s.layout.number(h)),
    }
}
/// Port kind at every vertex: 0 none, 1 generic, 2 to 6 a resource port.
fn ports(s: &State) -> [u8; 54] {
    let mut out = [0u8; 54];
    for (i, vertices) in topology().ports.iter().enumerate() {
        let kind = s.layout.ports[i] + 1;
        for &v in vertices {
            out[v as usize] = kind;
        }
    }
    out
}

/// One hex pattern.
pub fn hex_pattern(s: &State, seat: u8, h: u8) -> usize {
    let corners: [usize; 6] =
        std::array::from_fn(|i| corner(s, seat, topology().hex_vertices[h as usize][i]));
    let resource = s.layout.resource(h).map_or(0, |r| r as usize + 1);
    let robber = usize::from(s.robber == h);
    ((canonical_corners(&corners) * 6 + resource) * 6 + pips(s.layout.number(h))) * 2 + robber
}
/// The sorted triple of touching hexes and the building state at a vertex.
fn corner_parts(s: &State, seat: u8, v: u8) -> (usize, usize) {
    let t = topology();
    let mut codes = [36usize; 3];
    for (i, &h) in t.incident_hexes[v as usize].iter().take(3).enumerate() {
        codes[i] = hex_code(s, h);
    }
    codes.sort_unstable_by(|a, b| b.cmp(a));
    let hexes = (codes[0] * 37 + codes[1]) * 37 + codes[2];
    let building = match s.board.owner(v) {
        None => {
            if t.neighbors[v as usize]
                .iter()
                .any(|&n| s.board.building(n) != 0)
            {
                1
            } else {
                0
            }
        }
        Some(o) if o == seat => {
            if s.board.building(v) > 4 {
                3
            } else {
                2
            }
        }
        Some(_) => {
            if s.board.building(v) > 4 {
                5
            } else {
                4
            }
        }
    };
    (hexes, building)
}
/// One compact corner pattern: hexes and building state only.
pub fn corner_pattern(s: &State, seat: u8, v: u8) -> usize {
    let (hexes, building) = corner_parts(s, seat, v);
    hexes * 6 + building
}
/// One vertex pattern.
pub fn vertex_pattern(s: &State, seat: u8, v: u8, port: u8) -> usize {
    let t = topology();
    let (hexes, building) = corner_parts(s, seat, v);
    let mut roads = 0usize;
    for &e in &t.incident_edges[v as usize] {
        match s.board.road(e) {
            Some(o) if o == seat => roads |= 1,
            Some(_) => roads |= 2,
            None => {}
        }
    }
    ((hexes * 6 + building) * 4 + roads) * 7 + usize::from(port)
}
/// Production pips per resource for a seat, the robber's hex excluded, cities doubled.
pub fn production_pips(s: &State, seat: u8) -> [usize; 5] {
    let mut out = [0usize; 5];
    for h in 0..19u8 {
        if h == s.robber {
            continue;
        }
        let Some(r) = s.layout.resource(h) else {
            continue;
        };
        let p = pips(s.layout.number(h));
        for &v in &topology().hex_vertices[h as usize] {
            if s.board.owner(v) == Some(seat) {
                out[r as usize] += p * if s.board.building(v) > 4 { 2 } else { 1 };
            }
        }
    }
    out
}
fn production_pattern(pips: &[usize; 5]) -> usize {
    pips.iter().fold(0, |acc, &p| acc * 16 + p.min(15))
}
fn hand_pattern(s: &State, seat: u8) -> usize {
    let own = s.players[seat as usize];
    let hand = own
        .resources
        .0
        .iter()
        .fold(0usize, |acc, &n| acc * 5 + usize::from(n.min(4)));
    let total = usize::from(own.resources.total().min(8));
    let settlements_left = usize::from(5u8.saturating_sub(own.settlements));
    let cities_left = usize::from(4u8.saturating_sub(own.cities));
    ((hand * 9 + total) * 6 + settlements_left) * 5 + cities_left
}
fn opponent_scores(s: &State, seat: u8) -> (usize, usize) {
    let mut first = 0usize;
    let mut second = 0usize;
    for o in 0..s.settings.players {
        if o == seat {
            continue;
        }
        let pts = usize::from(public_points(s, o)).min(12);
        if pts > first {
            second = first;
            first = pts;
        } else if pts > second {
            second = pts;
        }
    }
    (first, second)
}
fn race_pattern(s: &State, seat: u8) -> usize {
    let own = usize::from(points(s, seat)).min(12);
    let (first, second) = opponent_scores(s, seat);
    let bucket = (s.turn as usize / 12).min(7);
    ((own * 13 + first) * 13 + second) * 8 + bucket
}
fn holder(award: Option<u8>, seat: u8) -> usize {
    match award {
        None => 0,
        Some(p) if p == seat => 1,
        Some(_) => 2,
    }
}
fn development_pattern(s: &State, seat: u8) -> usize {
    let own = s.players[seat as usize];
    let knights = usize::from(own.played_knights.min(3));
    let held = (0..4)
        .map(|i| usize::from(own.development[i] + own.new_development[i]))
        .sum::<usize>()
        .min(3);
    let trail = if own.roads >= 4 {
        usize::from(road_length(s, seat))
    } else {
        usize::from(own.roads)
    }
    .min(15);
    let roads_left = usize::from(15u8.saturating_sub(own.roads)).min(15);
    let deck = match s.deck_count() {
        0 => 0,
        1..=5 => 1,
        6..=15 => 2,
        _ => 3,
    };
    (((((knights * 4 + held) * 3 + holder(s.largest_army, seat)) * 3
        + holder(s.longest_road, seat))
        * 16
        + trail)
        * 16
        + roads_left)
        * 4
        + deck
}
fn opponent_pattern(s: &State, seat: u8, own_pips: usize) -> usize {
    let mut largest_hand = 0usize;
    let mut best_pips = 0usize;
    let mut count = 0usize;
    for o in 0..s.settings.players {
        if o == seat {
            continue;
        }
        count += 1;
        largest_hand = largest_hand.max(usize::from(s.players[o as usize].resources.total()));
        best_pips = best_pips.max(production_pips(s, o).iter().sum::<usize>());
    }
    let (first, _) = opponent_scores(s, seat);
    (((largest_hand.min(9) * 32 + best_pips.min(31)) * 3 + count.saturating_sub(1).min(2)) * 32
        + own_pips.min(31))
        * 13
        + first
}

/// Which families a layout includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub hex: bool,
    pub vertex: bool,
    pub corner: bool,
    pub portfolio: bool,
}
impl Layout {
    /// Parse a comma-separated list of `hex`, `vertex`, `corner`, and `portfolio`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut layout = Self {
            hex: false,
            vertex: false,
            corner: false,
            portfolio: false,
        };
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item {
                "hex" => layout.hex = true,
                "vertex" => layout.vertex = true,
                "corner" => layout.corner = true,
                "portfolio" => layout.portfolio = true,
                other => return Err(format!("unknown layout family {other:?}")),
            }
        }
        if !(layout.hex || layout.vertex || layout.corner || layout.portfolio) {
            return Err("layout names no tuple family".into());
        }
        Ok(layout)
    }
    pub fn spec(&self) -> String {
        let mut parts = Vec::new();
        if self.hex {
            parts.push("hex");
        }
        if self.vertex {
            parts.push("vertex");
        }
        if self.corner {
            parts.push("corner");
        }
        if self.portfolio {
            parts.push("portfolio");
        }
        parts.join(",")
    }
    /// (family, tables, entries per table), in feature order.
    pub fn families(&self) -> Vec<(&'static str, usize, usize)> {
        let mut out = Vec::new();
        if self.hex {
            out.push(("hex", 1, HEX_PATTERNS));
        }
        if self.vertex {
            out.push(("vertex", 1, VERTEX_PATTERNS));
        }
        if self.corner {
            out.push(("corner", 1, CORNER_PATTERNS));
        }
        if self.portfolio {
            out.push(("production", 1, PRODUCTION_PATTERNS));
            out.push(("hand", 1, HAND_PATTERNS));
            out.push(("race", 1, RACE_PATTERNS));
            out.push(("development", 1, DEVELOPMENT_PATTERNS));
            out.push(("opponents", 1, OPPONENT_PATTERNS));
        }
        out
    }
    pub fn total_entries(&self) -> usize {
        self.families().iter().map(|(_, t, e)| t * e).sum()
    }
    /// Active entries per position.
    pub fn active_count(&self) -> usize {
        usize::from(self.hex) * HEX_COUNT
            + usize::from(self.vertex) * VERTEX_COUNT
            + usize::from(self.corner) * CORNER_COUNT
            + usize::from(self.portfolio) * PORTFOLIO_COUNT
    }
}

/// Table offsets of a layout: the first entry of the hex table, the vertex table, and each
/// portfolio table, in family order.
#[derive(Debug, Clone, Copy)]
pub struct Offsets {
    pub hex: Option<usize>,
    pub vertex: Option<usize>,
    pub corner: Option<usize>,
    pub portfolio: Option<[usize; PORTFOLIO_COUNT]>,
}
impl Offsets {
    pub fn of(layout: &Layout) -> Self {
        let mut cursor = 0usize;
        let hex = layout.hex.then(|| {
            let at = cursor;
            cursor += HEX_PATTERNS;
            at
        });
        let vertex = layout.vertex.then(|| {
            let at = cursor;
            cursor += VERTEX_PATTERNS;
            at
        });
        let corner = layout.corner.then(|| {
            let at = cursor;
            cursor += CORNER_PATTERNS;
            at
        });
        let portfolio = layout.portfolio.then(|| {
            let sizes = [
                PRODUCTION_PATTERNS,
                HAND_PATTERNS,
                RACE_PATTERNS,
                DEVELOPMENT_PATTERNS,
                OPPONENT_PATTERNS,
            ];
            let mut out = [0usize; PORTFOLIO_COUNT];
            for (slot, size) in out.iter_mut().zip(sizes) {
                *slot = cursor;
                cursor += size;
            }
            out
        });
        Self {
            hex,
            vertex,
            corner,
            portfolio,
        }
    }
}

/// Active entries of a position for a seat, in fixed order: hexes 0 to 18, vertices 0 to 53,
/// then the five portfolio tables. Returns the count written.
pub fn features(layout: &Layout, off: &Offsets, s: &State, seat: u8, out: &mut [u32]) -> usize {
    let mut n = 0usize;
    if let Some(base) = off.hex {
        for h in 0..19u8 {
            out[n] = (base + hex_pattern(s, seat, h)) as u32;
            n += 1;
        }
    }
    if let Some(base) = off.vertex {
        let ports = ports(s);
        for v in 0..54u8 {
            out[n] = (base + vertex_pattern(s, seat, v, ports[v as usize])) as u32;
            n += 1;
        }
    }
    if let Some(base) = off.corner {
        for v in 0..54u8 {
            out[n] = (base + corner_pattern(s, seat, v)) as u32;
            n += 1;
        }
    }
    if let Some(bases) = off.portfolio {
        let pips = production_pips(s, seat);
        let own_pips: usize = pips.iter().sum();
        out[n] = (bases[0] + production_pattern(&pips)) as u32;
        out[n + 1] = (bases[1] + hand_pattern(s, seat)) as u32;
        out[n + 2] = (bases[2] + race_pattern(s, seat)) as u32;
        out[n + 3] = (bases[3] + development_pattern(s, seat)) as u32;
        out[n + 4] = (bases[4] + opponent_pattern(s, seat, own_pips)) as u32;
        n += PORTFOLIO_COUNT;
    }
    debug_assert_eq!(n, layout.active_count());
    n
}
