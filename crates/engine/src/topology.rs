use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

/// Stable geometry shared by every state. Coordinates use an integer pointy-hex lattice.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Topology {
    pub vertices: Vec<[i8; 2]>,
    pub edges: Vec<[u8; 2]>,
    pub hex_vertices: [[u8; 6]; 19],
    pub hex_centers: [[i8; 2]; 19],
    pub incident_edges: Vec<Vec<u8>>,
    pub incident_hexes: Vec<Vec<u8>>,
    pub neighbors: Vec<Vec<u8>>,
    pub ports: [[u8; 2]; 9],
    pub spiral: [u8; 19],
}
static TOPOLOGY: LazyLock<Topology> = LazyLock::new(generate);
pub fn topology() -> &'static Topology {
    &TOPOLOGY
}
fn generate() -> Topology {
    let axial: Vec<(i8, i8)> = (-2i8..=2)
        .flat_map(|r| (-2i8..=2).filter_map(move |q| ((q + r).abs() <= 2).then_some((q, r))))
        .collect();
    let corners = [(0, -2), (1, -1), (1, 1), (0, 2), (-1, 1), (-1, -1)];
    let centers: Vec<_> = axial.iter().map(|&(q, r)| [2 * q + r, 3 * r]).collect();
    let points: BTreeSet<_> = centers
        .iter()
        .flat_map(|&[x, y]| corners.map(|(dx, dy)| [x + dx, y + dy]))
        .collect();
    let vertices: Vec<_> = points.into_iter().collect();
    let ids: BTreeMap<_, _> = vertices
        .iter()
        .enumerate()
        .map(|(i, &p)| (p, i as u8))
        .collect();
    let mut hex_vertices = [[0; 6]; 19];
    let mut edge_set = BTreeSet::new();
    let mut counts = BTreeMap::new();
    for (h, &[x, y]) in centers.iter().enumerate() {
        hex_vertices[h] = corners.map(|(dx, dy)| ids[&[x + dx, y + dy]]);
        for i in 0..6 {
            let a = hex_vertices[h][i];
            let b = hex_vertices[h][(i + 1) % 6];
            let e = [a.min(b), a.max(b)];
            edge_set.insert(e);
            *counts.entry(e).or_insert(0) += 1;
        }
    }
    let edges: Vec<_> = edge_set.into_iter().collect();
    let mut incident_edges = vec![vec![]; 54];
    let mut incident_hexes = vec![vec![]; 54];
    let mut neighbors = vec![vec![]; 54];
    for (i, &[a, b]) in edges.iter().enumerate() {
        incident_edges[a as usize].push(i as u8);
        incident_edges[b as usize].push(i as u8);
        neighbors[a as usize].push(b);
        neighbors[b as usize].push(a);
    }
    for (h, vs) in hex_vertices.iter().enumerate() {
        for &v in vs {
            incident_hexes[v as usize].push(h as u8);
        }
    }
    let coastal: Vec<_> = edges.iter().filter(|e| counts[*e] == 1).copied().collect();
    let start = coastal.iter().flatten().copied().min().unwrap_or(0);
    let mut coast = vec![start];
    let mut previous = 255;
    let mut current = start;
    loop {
        let next = coastal
            .iter()
            .filter_map(|&[a, b]| {
                if a == current && b != previous {
                    Some(b)
                } else if b == current && a != previous {
                    Some(a)
                } else {
                    None
                }
            })
            .min()
            .unwrap_or(start);
        if next == start {
            break;
        }
        coast.push(next);
        previous = current;
        current = next;
    }
    let offsets = [0, 3, 7, 10, 13, 17, 20, 23, 27];
    let ports = offsets.map(|i| [coast[i], coast[(i + 1) % coast.len()]]);
    let mut spiral = Vec::new();
    for radius in [2, 1] {
        let (mut q, mut r) = (-radius, 0);
        for (dq, dr) in [(0, 1), (1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1)] {
            for _ in 0..radius {
                spiral.push(axial.iter().position(|&p| p == (q, r)).unwrap_or(0) as u8);
                q += dq;
                r += dr;
            }
        }
    }
    spiral.push(axial.iter().position(|&p| p == (0, 0)).unwrap_or(0) as u8);
    Topology {
        vertices,
        edges,
        hex_vertices,
        hex_centers: std::array::from_fn(|i| centers[i]),
        incident_edges,
        incident_hexes,
        neighbors,
        ports,
        spiral: std::array::from_fn(|i| spiral[i]),
    }
}
