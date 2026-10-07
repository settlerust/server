use super::*;
use settlers_engine::{Board, Phase, Settings, State, apply, legal_actions, topology};

fn fixture() -> State {
    let mut s = State::new(Settings::default(), [7; 32]).unwrap();
    let mut steps = 0;
    while s.turn < 12 && steps < 2000 {
        let p = match s.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => s.current,
        };
        let a = crate::v2::opponent::act(&s, p, 0);
        s = apply(&s, p, a)
            .unwrap_or_else(|_| apply(&s, p, *legal_actions(&s, p).first().unwrap()).unwrap());
        steps += 1;
    }
    s
}
#[test]
fn layouts_size_as_documented() {
    let full = Layout::parse("hex,vertex,corner,portfolio").unwrap();
    assert_eq!(full.active_count(), MAX_ACTIVE);
    assert_eq!(
        Layout::parse("hex,vertex,portfolio")
            .unwrap()
            .active_count(),
        78
    );
    assert_eq!(
        full.total_entries(),
        features::HEX_PATTERNS
            + features::VERTEX_PATTERNS
            + features::CORNER_PATTERNS
            + features::PRODUCTION_PATTERNS
            + features::HAND_PATTERNS
            + features::RACE_PATTERNS
            + features::DEVELOPMENT_PATTERNS
            + features::OPPONENT_PATTERNS
    );
    assert_eq!(Layout::parse(&full.spec()).unwrap(), full);
    assert!(Layout::parse("").is_err());
    assert!(Layout::parse("rows").is_err());
    assert_eq!(Layout::parse("portfolio").unwrap().active_count(), 5);
}
#[test]
fn features_are_distinct_and_in_range_for_every_seat() {
    let s = fixture();
    for spec in [
        "hex,vertex,corner,portfolio",
        "hex",
        "vertex",
        "corner",
        "portfolio",
    ] {
        let layout = Layout::parse(spec).unwrap();
        let model = Model::new(layout, 0.5, false);
        let mut out = [0u32; MAX_ACTIVE];
        for seat in 0..4 {
            let n = model.features(&s, seat, &mut out);
            assert_eq!(n, layout.active_count());
            assert!(out[..n].iter().all(|&i| (i as usize) < model.entries()));
            // Shared tables may repeat an entry (two identical corners); the portfolio
            // tables never do.
            if spec == "portfolio" {
                let mut seen: Vec<u32> = out[..n].to_vec();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), n);
            }
        }
    }
}
#[test]
fn value_reads_only_the_seat_information() {
    // Changing an opponent's hidden hand composition at a fixed total leaves the value
    // unchanged; changing the seat's own hand changes it once trained.
    let s = fixture();
    let layout = Layout::parse("hex,vertex,portfolio").unwrap();
    let model = Model::new(layout, 0.5, true);
    let mut out = [0u32; MAX_ACTIVE];
    let n = model.features(&s, 0, &mut out);
    let mut stats = UpdateStats::default();
    model.update(&out[..n], 1.0, 1.0, 1.0, &mut stats);
    let before = model.value(&s, 0);
    let mut other = s;
    let hand = &mut other.players[1].resources;
    if hand.total() >= 1 {
        let from = (0..5).find(|&r| hand.0[r] > 0).unwrap();
        let to = (from + 1) % 5;
        hand.0[from] -= 1;
        hand.0[to] += 1;
    }
    other.random.word += 1000;
    // Deck composition is hidden; only its total is public.
    let total = other.deck.iter().sum::<u8>();
    other.deck = [total.saturating_sub(4), 1, 1, 1, 1];
    if total < 4 {
        other.deck = [total, 0, 0, 0, 0];
    }
    assert_eq!(model.value(&other, 0).to_bits(), before.to_bits());
    assert!((before - 1.0).abs() < 1e-4, "{before}");
}
#[test]
fn hex_patterns_are_symmetric_under_rotation() {
    let s = fixture();
    for h in 0..19u8 {
        let mut t = s;
        // Rotate the corners: rebuild the board with each corner's building moved one step.
        let verts = topology().hex_vertices[h as usize];
        let mut ok = true;
        let mut rotated = Board::default();
        let mut buildings = [0u8; 54];
        let mut roads = [None; 72];
        for v in 0..54u8 {
            buildings[v as usize] = s.board.building(v);
        }
        for e in 0..72u8 {
            roads[e as usize] = s.board.road(e);
        }
        let originals: Vec<u8> = verts.iter().map(|&v| s.board.building(v)).collect();
        for i in 0..6 {
            buildings[verts[(i + 1) % 6] as usize] = originals[i];
        }
        if let Ok(b) = Board::from_public(&buildings, &roads) {
            rotated = b;
        } else {
            ok = false;
        }
        if ok {
            t.board = rotated;
            assert_eq!(
                features::hex_pattern(&s, 0, h),
                features::hex_pattern(&t, 0, h)
            );
        }
    }
}
#[test]
fn save_and_load_round_trip() {
    let layout = Layout::parse("portfolio").unwrap();
    let model = Model::new(layout, 0.5, true);
    let s = fixture();
    let mut out = [0u32; MAX_ACTIVE];
    let n = model.features(&s, 2, &mut out);
    let mut stats = UpdateStats::default();
    model.update(&out[..n], 0.9, 1.0, 1.0, &mut stats);
    let mut bytes = Vec::new();
    model.write_to(&mut bytes, true).unwrap();
    let loaded = Model::read_from(&mut std::io::Cursor::new(&bytes), false).unwrap();
    assert!(!loaded.trainable());
    assert_eq!(loaded.value(&s, 2).to_bits(), model.value(&s, 2).to_bits());
    assert_eq!(loaded.fingerprint(), model.fingerprint());
    let resumed = Model::read_from(&mut std::io::Cursor::new(&bytes), true).unwrap();
    assert!(resumed.trainable());
    assert_eq!(resumed.touched_entries(), n as u64);
}
#[test]
fn a_single_update_moves_the_prediction_by_alpha_delta() {
    let layout = Layout::parse("hex,vertex,portfolio").unwrap();
    let model = Model::new(layout, 0.0, true);
    let s = fixture();
    let mut out = [0u32; MAX_ACTIVE];
    let n = model.features(&s, 1, &mut out);
    let mut stats = UpdateStats::default();
    let delta = model.update(&out[..n], 0.8, 0.5, 10.0, &mut stats);
    assert!((delta - 0.8).abs() < 1e-6);
    let after = model.value_of(&out[..n]);
    assert!((after - 0.4).abs() < 1e-4, "{after}");
    // Repeated entries: a position whose 78 slots all name one entry still moves by
    // alpha * delta.
    let model = Model::new(layout, 0.0, true);
    let same = [out[0]; MAX_ACTIVE];
    let delta = model.update(&same, 1.0, 0.5, 10.0, &mut stats);
    assert!((delta - 1.0).abs() < 1e-6);
    let after = model.value_of(&same);
    assert!((after - 0.5).abs() < 1e-4, "{after}");
}
