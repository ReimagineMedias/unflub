//! Fixed-size latency histogram for the hook callback. Recording is O(1) and allocation-free,
//! so it is safe to call on every key.
//!
//! Values under 128 ns are exact; above that each power of two is split into 16 buckets, so a
//! reported percentile is at most ~6% below the true value. The maximum is tracked exactly.

use std::fmt;
use std::time::Duration;

const LINEAR: usize = 128;
const SUB_BITS: u32 = 4;
const SUB: usize = 1 << SUB_BITS;
// Exponents 7..=63 above the linear range.
const BUCKETS: usize = LINEAR + (64 - 7) * SUB;

#[derive(Clone)]
pub struct LatencyHistogram {
    counts: Box<[u64; BUCKETS]>,
    total: u64,
    max_ns: u64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            counts: Box::new([0; BUCKETS]),
            total: 0,
            max_ns: 0,
        }
    }
}

fn bucket_of(ns: u64) -> usize {
    if ns < LINEAR as u64 {
        return ns as usize;
    }
    let exp = 63 - ns.leading_zeros(); // >= 7
    let sub = ((ns >> (exp - SUB_BITS)) as usize) & (SUB - 1);
    LINEAR + (exp as usize - 7) * SUB + sub
}

/// Smallest value that falls in bucket `i`.
fn bucket_floor(i: usize) -> u64 {
    if i < LINEAR {
        return i as u64;
    }
    let exp = (i - LINEAR) / SUB + 7;
    let sub = ((i - LINEAR) % SUB) as u64;
    (1u64 << exp) | (sub << (exp as u32 - SUB_BITS))
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        self.counts[bucket_of(ns)] += 1;
        self.total += 1;
        self.max_ns = self.max_ns.max(ns);
    }

    pub fn count(&self) -> u64 {
        self.total
    }

    pub fn max(&self) -> Duration {
        Duration::from_nanos(self.max_ns)
    }

    /// The `p`th percentile (0.0..=1.0), rounded down to its bucket. `None` when empty.
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.total == 0 {
            return None;
        }
        let rank = ((p.clamp(0.0, 1.0) * self.total as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                return Some(Duration::from_nanos(bucket_floor(i).min(self.max_ns)));
            }
        }
        Some(self.max())
    }
}

impl fmt::Display for LatencyHistogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let us = |d: Option<Duration>| d.map_or(0.0, |d| d.as_secs_f64() * 1e6);
        write!(
            f,
            "n={} p50={:.1}us p99={:.1}us max={:.1}us",
            self.total,
            us(self.percentile(0.50)),
            us(self.percentile(0.99)),
            us(Some(self.max())),
        )
    }
}

impl fmt::Debug for LatencyHistogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_no_percentiles() {
        let h = LatencyHistogram::new();
        assert_eq!(h.percentile(0.5), None);
        assert_eq!(h.count(), 0);
    }

    #[test]
    fn buckets_are_monotonic_and_cover_their_floor() {
        let mut last = 0;
        for ns in (0..5_000u64).chain([1 << 20, 123_456_789, u64::MAX]) {
            let b = bucket_of(ns);
            assert!(b >= last, "ns {ns}");
            assert!(bucket_floor(b) <= ns, "ns {ns}");
            if b + 1 < BUCKETS {
                assert!(bucket_floor(b + 1) > ns, "ns {ns}");
            }
            last = b;
        }
        assert_eq!(bucket_of(u64::MAX), BUCKETS - 1);
    }

    #[test]
    fn percentiles_and_max() {
        let mut h = LatencyHistogram::new();
        // 98 fast keys at 5 us, one at 200 us, one at 3 ms.
        for _ in 0..98 {
            h.record(Duration::from_micros(5));
        }
        h.record(Duration::from_micros(200));
        h.record(Duration::from_millis(3));
        let p50 = h.percentile(0.5).unwrap().as_nanos() as f64;
        assert!((5_000.0 * 0.94..=5_000.0).contains(&p50), "{p50}");
        let p99 = h.percentile(0.99).unwrap().as_nanos() as f64;
        assert!((200_000.0 * 0.94..=200_000.0).contains(&p99), "{p99}");
        assert_eq!(h.max(), Duration::from_millis(3));
        assert!(h.percentile(1.0).unwrap().as_nanos() as f64 >= 3e6 * 0.94);
        assert_eq!(h.count(), 100);
        let s = h.to_string();
        assert!(s.starts_with("n=100 p50="), "{s}");
        assert!(s.ends_with("max=3000.0us"), "{s}");
    }

    #[test]
    fn small_values_are_exact() {
        let mut h = LatencyHistogram::new();
        h.record(Duration::from_nanos(42));
        assert_eq!(h.percentile(0.5), Some(Duration::from_nanos(42)));
    }
}
