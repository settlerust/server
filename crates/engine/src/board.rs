use crate::{Random, Resource, topology};
use serde::{Deserialize, Serialize};

/// 54 bytes: 54 four-bit intersections and 72 three-bit roads. No heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Board {
    vertices: [u8; 27],
    roads: [u8; 27],
}
impl Board {
    /// Reconstruct public occupancy for offline simulation, validating every packed value.
    pub fn from_public(buildings: &[u8], roads: &[Option<u8>]) -> Result<Self, crate::RuleError> {
        if buildings.len() != 54
            || roads.len() != 72
            || buildings.iter().any(|&b| b > 8)
            || roads.iter().flatten().any(|&p| p >= 4)
        {
            return Err(crate::RuleError::InvalidLocation);
        }
        let mut board = Self::default();
        for (v, &b) in buildings.iter().enumerate() {
            board.set_building(v as u8, b);
        }
        for (e, &p) in roads.iter().enumerate() {
            if let Some(p) = p {
                board.set_road(e as u8, p);
            }
        }
        Ok(board)
    }
    /// 0 is empty, 1..=4 are settlements, 5..=8 are cities.
    pub fn building(&self, vertex: u8) -> u8 {
        self.vertices
            .get(vertex as usize / 2)
            .map_or(0, |b| (b >> ((vertex % 2) * 4)) & 15)
    }
    /// Player seat owning an intersection, or None.
    pub fn owner(&self, vertex: u8) -> Option<u8> {
        let value = self.building(vertex);
        if value == 0 {
            None
        } else {
            Some((value - 1) % 4)
        }
    }
    pub fn road(&self, edge: u8) -> Option<u8> {
        if edge >= 72 {
            return None;
        }
        let bit = edge as usize * 3;
        let pair = self.roads[bit / 8] as u16
            | ((self.roads.get(bit / 8 + 1).copied().unwrap_or(0) as u16) << 8);
        let value = ((pair >> (bit % 8)) & 7) as u8;
        value.checked_sub(1)
    }
    pub(crate) fn set_building(&mut self, vertex: u8, value: u8) {
        let shift = (vertex % 2) * 4;
        let byte = &mut self.vertices[vertex as usize / 2];
        *byte = (*byte & !(15 << shift)) | (value << shift);
    }
    pub(crate) fn set_road(&mut self, edge: u8, player: u8) {
        let bit = edge as usize * 3;
        for i in 0..3 {
            let at = bit + i;
            let mask = 1 << (at % 8);
            self.roads[at / 8] =
                (self.roads[at / 8] & !mask) | ((((player + 1) >> i) & 1) << (at % 8));
        }
    }
}

/// Hex byte: high nibble is dice total, low nibble resource + 1 (0 = desert).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub hexes: [u8; 19],
    pub ports: [u8; 9],
}
impl Layout {
    pub fn generate(random: &mut Random) -> Self {
        let mut resources = [0, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5];
        random.shuffle(&mut resources);
        let numbers = [5, 2, 6, 3, 8, 10, 9, 12, 11, 4, 8, 10, 9, 4, 5, 6, 3, 11];
        let mut hexes = resources;
        let mut n = 0;
        for &h in &topology().spiral {
            if hexes[h as usize] != 0 {
                hexes[h as usize] |= numbers[n] << 4;
                n += 1;
            }
        }
        let mut ports = [0, 0, 0, 0, 1, 2, 3, 4, 5];
        random.shuffle(&mut ports);
        Self { hexes, ports }
    }
    pub fn resource(&self, hex: u8) -> Option<Resource> {
        self.hexes
            .get(hex as usize)
            .and_then(|b| Resource::ALL.get((b & 15).wrapping_sub(1) as usize))
            .copied()
    }
    pub fn number(&self, hex: u8) -> u8 {
        self.hexes.get(hex as usize).copied().unwrap_or(0) >> 4
    }
    pub fn desert(&self) -> u8 {
        self.hexes.iter().position(|b| b & 15 == 0).unwrap_or(0) as u8
    }
}
