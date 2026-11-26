//! Weighted Damerau–Levenshtein distance tuned for typing slips.
//!
//! `typed` is what the user typed, `cand` a dictionary word, both lowercase ASCII. A deletion
//! means the user typed an extra character; an insertion means they dropped one.

use crate::layout::Adjacency;

pub const MAX_LEN: usize = 32;
const DIM: usize = MAX_LEN + 2;
const INF: f32 = 1.0e9;

/// Added to every edit, so two cheap slips cost more than one ordinary one.
pub const EDIT_BASE: f32 = 0.5;
pub const SUB: f32 = 1.0;
pub const SUB_NEIGHBOUR: f32 = 0.5;
const SUB_LAYOUT_SWAP: f32 = 0.25;
pub const SUB_VOWEL: f32 = 0.5;
pub const TRANSPOSE: f32 = 0.6;
pub const INSERT: f32 = 0.5;
pub const DELETE: f32 = 0.8;
pub const DELETE_NEIGHBOUR: f32 = 0.6;
/// A doubled letter typed once, or a single letter typed twice.
pub const DOUBLE: f32 = 0.4;
pub const APOSTROPHE: f32 = 0.1;
/// Extra cost for editing the first letter, except a slip to a neighbouring key; people rarely
/// misspell the start of a word.
pub const FIRST_LETTER: f32 = 0.4;

fn is_vowel(c: u8) -> bool {
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

fn sub_cost(adj: &Adjacency, a: u8, b: u8) -> f32 {
    EDIT_BASE
        + if adj.reciprocal_swap(a, b) {
            SUB_LAYOUT_SWAP
        } else if adj.adjacent(a, b) {
            SUB_NEIGHBOUR
        } else if is_vowel(a) && is_vowel(b) {
            SUB_VOWEL
        } else {
            SUB
        }
}

/// Cost of the typed character at `i` being extra.
fn del_cost(adj: &Adjacency, a: &[u8], i: usize) -> f32 {
    let c = a[i];
    let first = if i == 0 { FIRST_LETTER } else { 0.0 };
    let prev = if i > 0 { Some(a[i - 1]) } else { None };
    let next = a.get(i + 1).copied();
    let cost = if c == b'\'' {
        APOSTROPHE
    } else if prev == Some(c) || next == Some(c) {
        DOUBLE
    } else if prev.is_some_and(|p| adj.adjacent(c, p)) || next.is_some_and(|n| adj.adjacent(c, n)) {
        DELETE_NEIGHBOUR
    } else {
        DELETE
    };
    EDIT_BASE + cost + first
}

/// Cost of the candidate character at `j` having been dropped.
fn ins_cost(b: &[u8], j: usize) -> f32 {
    let c = b[j];
    let first = if j == 0 { FIRST_LETTER } else { 0.0 };
    let cost = if c == b'\'' {
        APOSTROPHE
    } else if (j > 0 && b[j - 1] == c) || b.get(j + 1) == Some(&c) {
        DOUBLE
    } else {
        INSERT
    };
    EDIT_BASE + cost + first
}

/// Weighted unrestricted Damerau–Levenshtein (Lowrance–Wagner). Returns `None` when the cost
/// exceeds `limit` or either word is longer than [`MAX_LEN`].
pub fn weighted(adj: &Adjacency, typed: &[u8], cand: &[u8], limit: f32) -> Option<f32> {
    let (n, m) = (typed.len(), cand.len());
    if n > MAX_LEN || m > MAX_LEN {
        return None;
    }
    // d[(i + 1) * DIM + (j + 1)] is the cost of typed[..i] -> cand[..j]; row/column 0 is the
    // "minus one" sentinel the transposition rule needs.
    let mut d = [INF; DIM * DIM];
    let at = |i: usize, j: usize| (i + 1) * DIM + (j + 1);
    d[at(0, 0)] = 0.0;
    for i in 1..=n {
        d[at(i, 0)] = d[at(i - 1, 0)] + del_cost(adj, typed, i - 1);
    }
    for j in 1..=m {
        d[at(0, j)] = d[at(0, j - 1)] + ins_cost(cand, j - 1);
    }
    let mut last_row = [0u8; 128];
    // Lower bound on any later transposition, which can jump back to an earlier row:
    // min over rows r < i of (min of row r) + DELETE * (i - 1 - r).
    let mut prev_row_min = 0.0f32;
    let mut jump_floor = f32::INFINITY;
    for i in 1..=n {
        let a = typed[i - 1];
        let mut last_col = 0usize;
        let mut row_min = d[at(i, 0)];
        for j in 1..=m {
            let b = cand[j - 1];
            let k = if b < 128 {
                last_row[b as usize] as usize
            } else {
                0
            };
            let l = last_col;
            let sub = if a == b {
                last_col = j;
                0.0
            } else {
                let cost = sub_cost(adj, a, b);
                let first = i == 1 && j == 1 && cost > EDIT_BASE + SUB_NEIGHBOUR;
                cost + if first { FIRST_LETTER } else { 0.0 }
            };
            let mut v = d[at(i - 1, j - 1)] + sub;
            v = v.min(d[at(i, j - 1)] + ins_cost(cand, j - 1));
            v = v.min(d[at(i - 1, j)] + del_cost(adj, typed, i - 1));
            if k > 0 && l > 0 {
                let between = (i - k - 1) as f32 * (EDIT_BASE + DELETE)
                    + (j - l - 1) as f32 * (EDIT_BASE + INSERT);
                v = v.min(d[at(k - 1, l - 1)] + between + EDIT_BASE + TRANSPOSE);
            }
            d[at(i, j)] = v;
            row_min = row_min.min(v);
        }
        jump_floor = (jump_floor + EDIT_BASE + DELETE).min(prev_row_min);
        if row_min > limit && jump_floor + EDIT_BASE + TRANSPOSE > limit {
            return None;
        }
        prev_row_min = row_min;
        if a < 128 {
            last_row[a as usize] = i as u8;
        }
    }
    let cost = d[at(n, m)];
    (cost <= limit).then_some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Adjacency;

    fn cost(a: &str, b: &str) -> f32 {
        let adj = Adjacency::default();
        weighted(&adj, a.as_bytes(), b.as_bytes(), 10.0).unwrap()
    }

    #[test]
    fn typing_slips_are_cheap() {
        assert_eq!(cost("the", "the"), 0.0);
        let one = |c: f32| EDIT_BASE + c;
        assert!((cost("teh", "the") - one(TRANSPOSE)).abs() < 1e-6);
        assert!((cost("couls", "could") - one(SUB_NEIGHBOUR)).abs() < 1e-6);
        assert!((cost("wouldnt", "wouldn't") - one(APOSTROPHE)).abs() < 1e-6);
        assert!((cost("aautocorrect", "autocorrect") - one(DOUBLE)).abs() < 1e-6);
        assert!((cost("begining", "beginning") - one(DOUBLE)).abs() < 1e-6);
        // Transposition with a letter dropped in between: tk -> ket.
        assert!((cost("martking", "marketing") - one(TRANSPOSE) - one(INSERT)).abs() < 1e-6);
        assert!(cost("cat", "dog") > 2.5);
    }

    #[test]
    fn cutoff_keeps_transpositions() {
        let adj = Adjacency::default();
        assert!(weighted(&adj, b"hte", b"the", EDIT_BASE + TRANSPOSE).is_some());
    }

    #[test]
    fn limit_cuts_off() {
        let adj = Adjacency::default();
        assert!(weighted(&adj, b"abcdef", b"uvwxyz", 1.5).is_none());
    }
}
