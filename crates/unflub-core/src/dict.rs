//! Bundled word list: unigram and bigram frequencies plus a symmetric-delete (SymSpell) index
//! for finding every word within edit distance 2 of a typo.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

const UNIGRAMS: &str = include_str!("../data/frequency_dictionary_en_82_765.txt");
const BIGRAMS: &str = include_str!("../data/frequency_bigramdictionary_en_243_342.txt");
const EXTRA_WORDS: &str = include_str!("../data/extra_words.txt");
const CONTRACTIONS: &str = include_str!("../data/contractions.txt");

/// Longest dictionary word kept, in bytes.
const MAX_WORD: usize = 32;
/// Only the first `PREFIX` letters of a word are indexed, as in SymSpell.
const PREFIX: usize = 7;
const BUCKET_BITS: u32 = 19;
const CONTRACTION_BOOST: f64 = 10.0;

/// A frequency dictionary with a delete index. Built once and shared between engines.
pub struct Dictionary {
    text: String,
    offsets: Vec<u32>,
    /// Natural log of each word's relative frequency.
    log_p: Vec<f32>,
    /// Open-addressed table of word ids + 1 (0 is empty), keyed by word hash.
    table: Vec<u32>,
    /// Delete index in CSR form: `del_ids[del_start[b]..del_start[b + 1]]` are the ids of words
    /// with a delete hashing to bucket `b`. Collisions only add candidates, never lose them.
    del_start: Vec<u32>,
    del_ids: Vec<u32>,
    /// Sorted `(first_id << 32) | second_id` keys with P(second | first).
    bigram_keys: Vec<u64>,
    bigram_p: Vec<f32>,
}

fn hash(bytes: impl IntoIterator<Item = u8>) -> u32 {
    // FNV-1a, then a final mix so low bits are usable as a bucket index.
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// Calls `f` with the hash of every string reachable from `w` by deleting up to two
/// characters, including `w` itself. May repeat hashes.
fn for_each_delete(w: &[u8], mut f: impl FnMut(u32)) {
    let n = w.len();
    f(hash(w.iter().copied()));
    for i in 0..n {
        let skip1 = w
            .iter()
            .enumerate()
            .filter(|&(k, _)| k != i)
            .map(|(_, &b)| b);
        f(hash(skip1));
        for j in i + 1..n {
            let skip2 = w
                .iter()
                .enumerate()
                .filter(|&(k, _)| k != i && k != j)
                .map(|(_, &b)| b);
            f(hash(skip2));
        }
    }
}

fn is_word(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= MAX_WORD
        && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'\'')
        && w.as_bytes()[0] != b'\''
}

fn parse_counts(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.rsplit_once(' '))
}

impl Dictionary {
    /// The bundled English dictionary, built on first use (tens of milliseconds in release).
    pub fn english() -> Arc<Dictionary> {
        static ENGLISH: OnceLock<Arc<Dictionary>> = OnceLock::new();
        ENGLISH
            .get_or_init(|| {
                Arc::new(Dictionary::build(
                    UNIGRAMS,
                    BIGRAMS,
                    EXTRA_WORDS,
                    CONTRACTIONS,
                ))
            })
            .clone()
    }

    fn build<'a>(
        unigrams: &'a str,
        bigrams: &'a str,
        extra: &'a str,
        contractions: &'a str,
    ) -> Self {
        fn add<'a>(
            w: &'a str,
            c: f64,
            counts: &mut Vec<(&'a str, f64)>,
            index: &mut HashMap<&'a str, usize>,
        ) {
            let w = w.trim();
            if !is_word(w) {
                return;
            }
            match index.get(w) {
                Some(&i) => counts[i].1 = counts[i].1.max(c),
                None => {
                    index.insert(w, counts.len());
                    counts.push((w, c));
                }
            }
        }
        let mut counts: Vec<(&'a str, f64)> = Vec::with_capacity(90_000);
        let mut index: HashMap<&'a str, usize> = HashMap::with_capacity(90_000);
        for (w, c) in parse_counts(unigrams).chain(parse_counts(extra)) {
            if let Ok(c) = c.trim().parse::<f64>() {
                add(w, c, &mut counts, &mut index);
            }
        }

        let uni_total: f64 = counts.iter().map(|&(_, c)| c).sum();
        let mut bigram_raw: HashMap<(usize, usize), f64> = HashMap::with_capacity(250_000);
        let mut bi_total = 0.0;
        for line in bigrams.lines() {
            let mut parts = line.split_ascii_whitespace();
            let (Some(a), Some(b), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Ok(c) = c.parse::<f64>() else { continue };
            bi_total += c;
            if let (Some(&ia), Some(&ib)) = (index.get(a), index.get(b)) {
                bigram_raw.insert((ia, ib), c);
            }
        }
        // The bigram list comes from a larger corpus than the unigram list.
        let bi_scale = if uni_total > 0.0 {
            bi_total / uni_total
        } else {
            1.0
        };

        // Contractions get the frequency of what they stand for, boosted because typed text
        // uses them far more than the books the counts come from.
        for line in contractions.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_ascii_whitespace();
            let Some(word) = parts.next() else { continue };
            let expansion: Vec<&str> = parts.collect();
            let c = match expansion.as_slice() {
                [one] => index.get(one).map(|&i| counts[i].1),
                [a, b] => match (index.get(a), index.get(b)) {
                    (Some(&ia), Some(&ib)) => bigram_raw.get(&(ia, ib)).map(|c| c / bi_scale),
                    _ => None,
                },
                _ => None,
            };
            if let Some(c) = c {
                add(word, c * CONTRACTION_BOOST, &mut counts, &mut index);
            }
        }

        let uni_total: f64 = counts.iter().map(|&(_, c)| c).sum();
        let mut text = String::with_capacity(counts.iter().map(|(w, _)| w.len()).sum());
        let mut offsets = Vec::with_capacity(counts.len() + 1);
        let mut log_p = Vec::with_capacity(counts.len());
        for &(w, c) in &counts {
            offsets.push(text.len() as u32);
            text.push_str(w);
            log_p.push((c.max(1.0) / uni_total).ln() as f32);
        }
        offsets.push(text.len() as u32);

        let mut table = vec![0u32; (counts.len() * 2).next_power_of_two()];
        let mask = table.len() - 1;
        for (id, &(w, _)) in counts.iter().enumerate() {
            let mut slot = hash(w.bytes()) as usize & mask;
            while table[slot] != 0 {
                slot = (slot + 1) & mask;
            }
            table[slot] = id as u32 + 1;
        }

        let (del_start, del_ids) = Self::build_deletes(&counts);

        let mut bigrams: Vec<(u64, f32)> = bigram_raw
            .into_iter()
            .map(|((a, b), c)| {
                let p_ab = c / bi_total;
                let p_a = counts[a].1 / uni_total;
                let cond = (p_ab / p_a).min(1.0) as f32;
                (((a as u64) << 32) | b as u64, cond)
            })
            .collect();
        bigrams.sort_unstable_by_key(|&(k, _)| k);
        let (bigram_keys, bigram_p) = bigrams.into_iter().unzip();

        Self {
            text,
            offsets,
            log_p,
            table,
            del_start,
            del_ids,
            bigram_keys,
            bigram_p,
        }
    }

    fn build_deletes(counts: &[(&str, f64)]) -> (Vec<u32>, Vec<u32>) {
        let buckets = 1usize << BUCKET_BITS;
        let mask = buckets as u32 - 1;
        let mut scratch: Vec<u32> = Vec::with_capacity(64);
        let word_buckets = |w: &str, scratch: &mut Vec<u32>| {
            scratch.clear();
            let w = &w.as_bytes()[..w.len().min(PREFIX)];
            for_each_delete(w, |h| scratch.push(h & mask));
            scratch.sort_unstable();
            scratch.dedup();
        };
        let mut start = vec![0u32; buckets + 1];
        for &(w, _) in counts {
            word_buckets(w, &mut scratch);
            for &b in &scratch {
                start[b as usize + 1] += 1;
            }
        }
        for b in 0..buckets {
            start[b + 1] += start[b];
        }
        let mut fill = start.clone();
        let mut ids = vec![0u32; start[buckets] as usize];
        for (id, &(w, _)) in counts.iter().enumerate() {
            word_buckets(w, &mut scratch);
            for &b in &scratch {
                ids[fill[b as usize] as usize] = id as u32;
                fill[b as usize] += 1;
            }
        }
        (start, ids)
    }

    /// Number of words.
    pub fn word_count(&self) -> usize {
        self.log_p.len()
    }

    pub(crate) fn word(&self, id: u32) -> &str {
        let (s, e) = (self.offsets[id as usize], self.offsets[id as usize + 1]);
        &self.text[s as usize..e as usize]
    }

    pub(crate) fn log_p(&self, id: u32) -> f32 {
        self.log_p[id as usize]
    }

    /// Id of a lowercase word, if known.
    pub(crate) fn id(&self, w: &[u8]) -> Option<u32> {
        let mask = self.table.len() - 1;
        let mut slot = hash(w.iter().copied()) as usize & mask;
        loop {
            let v = self.table[slot];
            if v == 0 {
                return None;
            }
            if self.word(v - 1).as_bytes() == w {
                return Some(v - 1);
            }
            slot = (slot + 1) & mask;
        }
    }

    /// P(second | first) from the bigram list, if the pair is listed.
    pub(crate) fn bigram(&self, first: u32, second: u32) -> Option<f32> {
        let key = ((first as u64) << 32) | second as u64;
        self.bigram_keys
            .binary_search(&key)
            .ok()
            .map(|i| self.bigram_p[i])
    }

    /// Pushes ids of words that may be within edit distance 2 of `typed` (lowercase) into
    /// `out`, deduplicated. Callers must verify the distance.
    pub(crate) fn candidates(&self, typed: &[u8], out: &mut Vec<u32>) {
        out.clear();
        let mask = (1u32 << BUCKET_BITS) - 1;
        let prefix = &typed[..typed.len().min(PREFIX)];
        let mut buckets = [0u32; 64];
        let mut nb = 0;
        for_each_delete(prefix, |h| {
            if nb < buckets.len() {
                buckets[nb] = h & mask;
                nb += 1;
            }
        });
        let buckets = &mut buckets[..nb];
        buckets.sort_unstable();
        let mut last = u32::MAX;
        for &b in buckets.iter() {
            if b == last {
                continue;
            }
            last = b;
            let (s, e) = (self.del_start[b as usize], self.del_start[b as usize + 1]);
            out.extend_from_slice(&self.del_ids[s as usize..e as usize]);
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Approximate heap size in bytes, for reporting.
    pub fn heap_bytes(&self) -> usize {
        self.text.len()
            + self.offsets.len() * 4
            + self.log_p.len() * 4
            + self.table.len() * 4
            + self.del_start.len() * 4
            + self.del_ids.len() * 4
            + self.bigram_keys.len() * 12
    }

    /// Number of word pairs with a bigram probability.
    pub fn bigram_count(&self) -> usize {
        self.bigram_keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_dictionary() {
        let d = Dictionary::build(
            "the 100\nthen 10\nhello 5\n",
            "hello there 50\nthe then 7\n",
            "ok 3\n",
            "can't cannot\n",
        );
        assert_eq!(d.word_count(), 4);
        assert!(d.id(b"then").is_some());
        assert!(d.id(b"ok").is_some());
        assert!(d.id(b"teh").is_none());
        let mut out = Vec::new();
        d.candidates(b"teh", &mut out);
        let words: Vec<&str> = out.iter().map(|&i| d.word(i)).collect();
        assert!(words.contains(&"the"));
        let (t, n) = (d.id(b"the").unwrap(), d.id(b"then").unwrap());
        assert!(d.bigram(t, n).is_some());
        assert!(d.bigram(n, t).is_none());
    }
}
