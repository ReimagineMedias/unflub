//! Physical keyboard layouts, used to make slips to a neighbouring key cheap.

/// A keyboard layout as rows of keys plus each row's horizontal stagger (in key widths).
pub struct Layout {
    pub rows: [&'static str; 4],
    pub stagger: [f32; 4],
}

pub const QWERTY: Layout = Layout {
    rows: ["1234567890-=", "qwertyuiop[]", "asdfghjkl;'", "zxcvbnm,./"],
    stagger: [0.0, 0.5, 0.75, 1.25],
};

/// Keys whose centres are closer than this (in key widths) count as neighbours.
const NEIGHBOUR_DISTANCE: f32 = 1.3;

/// Precomputed ASCII neighbour table for one layout.
pub struct Adjacency {
    bits: [u128; 128],
}

impl Adjacency {
    pub fn new(layout: &Layout) -> Self {
        let mut pos: [Option<(f32, f32)>; 128] = [None; 128];
        for (r, row) in layout.rows.iter().enumerate() {
            for (c, ch) in row.bytes().enumerate() {
                if ch < 128 {
                    pos[ch as usize] = Some((c as f32 + layout.stagger[r], r as f32));
                }
            }
        }
        let mut bits = [0u128; 128];
        for a in 0..128 {
            let Some((ax, ay)) = pos[a] else { continue };
            for (b, p) in pos.iter().enumerate() {
                let Some((bx, by)) = *p else { continue };
                let d = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
                if a != b && d < NEIGHBOUR_DISTANCE {
                    bits[a] |= 1u128 << b;
                }
            }
        }
        Self { bits }
    }

    #[inline]
    pub fn adjacent(&self, a: u8, b: u8) -> bool {
        a < 128 && b < 128 && self.bits[a as usize] & (1u128 << b) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwerty_neighbours() {
        let adj = Adjacency::new(&QWERTY);
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
