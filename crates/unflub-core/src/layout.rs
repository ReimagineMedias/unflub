//! Physical keyboard layouts, used to make slips to a neighbouring key cheap.

const QWERTY: [&[u8]; 4] = [
    b"1234567890-=",
    b"qwertyuiop[]",
    b"asdfghjkl;'",
    b"zxcvbnm,./",
];
const STAGGER: [f32; 4] = [0.0, 0.5, 0.75, 1.25];

/// Keys whose centres are closer than this (in key widths) count as neighbours.
const NEIGHBOUR_DISTANCE: f32 = 1.3;

/// Precomputed ASCII neighbour table for one layout.
pub struct Adjacency {
    bits: [u128; 128],
    swaps: [Option<u8>; 26],
}

impl Default for Adjacency {
    fn default() -> Self {
        Self::from_key_positions(QWERTY.iter().enumerate().flat_map(|(row, keys)| {
            keys.iter()
                .enumerate()
                .map(move |(col, &key)| (key, row, col as i8))
        }))
    }
}

impl Adjacency {
    /// Build from physical key positions as (lowercase ASCII key, row, column).
    /// Row 0 starts at the number keys. Rows 1 to 3 start at Q, A and Z on a US board.
    /// Unknown keys can be omitted.
    pub fn from_key_positions(positions: impl IntoIterator<Item = (u8, usize, i8)>) -> Self {
        let mut pos = [None; 128];
        for (key, row, col) in positions {
            if key.is_ascii() && row < STAGGER.len() {
                pos[key as usize] = Some((row, col));
            }
        }
        let mut bits = [0u128; 128];
        for a in 0..128 {
            let Some((ar, ac)) = pos[a] else { continue };
            let ax = f32::from(ac) + STAGGER[ar];
            for (b, p) in pos.iter().enumerate() {
                let Some((br, bc)) = *p else { continue };
                let bx = f32::from(bc) + STAGGER[br];
                let d = ((ax - bx).powi(2) + (ar as f32 - br as f32).powi(2)).sqrt();
                if (ar, ac) != (br, bc) && d < NEIGHBOUR_DISTANCE {
                    bits[a] |= 1u128 << b;
                }
            }
        }
        // These reciprocal moves account for muscle-memory slips when switching between
        // QWERTY and AZERTY or QWERTZ. Keep them separate from physical neighbours.
        let mut swaps = [None; 26];
        for (a, b, a_us, b_us) in [
            (b'a', b'q', (2, 0), (1, 0)),
            (b'z', b'w', (3, 0), (1, 1)),
            (b'z', b'y', (3, 0), (1, 5)),
        ] {
            if pos[a as usize] == Some(b_us) && pos[b as usize] == Some(a_us) {
                swaps[(a - b'a') as usize] = Some(b);
                swaps[(b - b'a') as usize] = Some(a);
            }
        }
        Self { bits, swaps }
    }

    #[inline]
    pub fn adjacent(&self, a: u8, b: u8) -> bool {
        a < 128 && b < 128 && self.bits[a as usize] & (1u128 << b) != 0
    }

    pub(crate) fn reciprocal_swap(&self, a: u8, b: u8) -> bool {
        a.is_ascii_lowercase() && self.swaps[(a - b'a') as usize] == Some(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwerty_neighbours() {
        let adj = Adjacency::default();
        for (a, ns) in [("s", "awedzx"), ("g", "ftyhvb"), ("q", "wa"), ("m", "njk,")] {
            let a = a.as_bytes()[0];
            for n in ns.bytes() {
                assert!(adj.adjacent(a, n), "{} {}", a as char, n as char);
            }
        }
        assert!(!adj.adjacent(b'a', b'd'));
        assert!(!adj.adjacent(b't', b'k'));
        assert!(!adj.adjacent(b'e', b'e'));
    }
}
