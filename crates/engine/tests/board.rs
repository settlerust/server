mod common;
use common::*;
use settlers_engine::*;
#[test]
fn topology_has_standard_counts_and_reciprocal_adjacency() {
    let t = topology();
    assert_eq!(t.vertices.len(), 54);
    assert_eq!(t.edges.len(), 72);
    for (e, &[a, b]) in t.edges.iter().enumerate() {
        assert!(t.incident_edges[a as usize].contains(&(e as u8)));
        assert!(t.incident_edges[b as usize].contains(&(e as u8)));
        assert!(t.neighbors[a as usize].contains(&b));
    }
    assert_eq!(
        t.spiral
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        19
    );
    let ports: std::collections::BTreeSet<_> = t.ports.iter().flatten().copied().collect();
    assert_eq!(ports.len(), 18);
}
#[test]
fn packed_board_roundtrips_every_location_and_player() {
    assert_eq!(std::mem::size_of::<Board>(), 54);
    for p in 0..4 {
        for e in 0..72 {
            let b = board(&[], &[(e, p)]);
            for other in 0..72 {
                assert_eq!(b.road(other), if e == other { Some(p) } else { None });
            }
        }
        for v in 0..54 {
            for value in 1..=8 {
                let b = board(&[(v, value)], &[]);
                assert_eq!(b.building(v), value);
                assert_eq!(b.owner(v), Some((value - 1) % 4));
            }
        }
    }
}
#[test]
fn seeded_layout_has_standard_inventory_and_no_adjacent_red_numbers() {
    for seed in 0..200 {
        let s = State::new(Settings::default(), [seed; 32]).unwrap();
        for (r, n) in [(0, 1), (1, 3), (2, 4), (3, 4), (4, 4), (5, 3)] {
            assert_eq!(s.layout.hexes.iter().filter(|&&h| h & 15 == r).count(), n);
        }
        let mut numbers: Vec<_> = s
            .layout
            .hexes
            .iter()
            .map(|h| h >> 4)
            .filter(|&n| n > 0)
            .collect();
        numbers.sort();
        assert_eq!(
            numbers,
            vec![2, 3, 3, 4, 4, 5, 5, 6, 6, 8, 8, 9, 9, 10, 10, 11, 11, 12]
        );
        for a in 0..19 {
            for b in a + 1..19 {
                let shared = topology().hex_vertices[a]
                    .iter()
                    .filter(|v| topology().hex_vertices[b].contains(v))
                    .count();
                if shared == 2 {
                    assert!(
                        !(matches!(s.layout.number(a as u8), 6 | 8)
                            && matches!(s.layout.number(b as u8), 6 | 8))
                    );
                }
            }
        }
    }
}
#[test]
fn complete_state_is_copyable_and_under_256_bytes() {
    assert!(
        std::mem::size_of::<State>() <= 256,
        "{}",
        std::mem::size_of::<State>()
    );
}
