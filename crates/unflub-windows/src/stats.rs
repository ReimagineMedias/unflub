//! Fixed-size latency histogram for the hook callback. Recording is O(1) and allocation-free,
//! so it is safe to call on every key.
//!
//! Values under 128 ns are exact; above that each power of two is split into 16 buckets, so a
//! reported percentile is at most ~6% below the true value. The maximum is tracked exactly.
//!
//! [`UsageCounters`] collects what the app's anonymous usage stats need from the hook thread.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Timing samples kept between two [`UsageCounters::take`] calls (the engine takes them every
/// few seconds). More are dropped rather than grow the buffer on the hook thread.
pub const SAMPLE_CAP: usize = 4096;

/// Counts and timings the hook thread (and the spell checker on it) adds to, for the app's
/// anonymous usage stats. Recording never allocates and never waits: counts are relaxed atomic
/// adds, and a timing sample is dropped if the buffer is full or another thread is taking it
/// at that moment (a swap of two pointers).
#[derive(Debug)]
pub struct UsageCounters {
    /// Distinct minutes with at least one key typed (`GetTickCount` minutes).
    pub active_minutes: AtomicU64,
    /// Words that ended in a password field, or one not yet known to be safe.
    pub skipped_secure: AtomicU64,
    decide_all_us: Mutex<Vec<u32>>,
    spellcheck_us: Mutex<Vec<u32>>,
}

/// What [`UsageCounters::take`] hands over.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UsageTaken {
    pub active_minutes: u64,
    pub skipped_secure: u64,
    /// Microseconds of every corrector call on a word-ending key, fixed or not.
    pub decide_all_us: Vec<u32>,
    /// Microseconds of each `ISpellChecker::Check` call.
    pub spellcheck_us: Vec<u32>,
}

impl Default for UsageCounters {
    fn default() -> Self {
        Self {
            active_minutes: AtomicU64::new(0),
            skipped_secure: AtomicU64::new(0),
            decide_all_us: Mutex::new(Vec::with_capacity(SAMPLE_CAP)),
            spellcheck_us: Mutex::new(Vec::with_capacity(SAMPLE_CAP)),
        }
    }
}

fn micros(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

fn push(samples: &Mutex<Vec<u32>>, d: Duration) {
    if let Ok(mut v) = samples.try_lock() {
        // Never past the capacity reserved off the hook thread, so this never allocates.
        if v.len() < v.capacity().min(SAMPLE_CAP) {
            v.push(micros(d));
        }
    }
}

fn take_samples(samples: &Mutex<Vec<u32>>) -> Vec<u32> {
    let fresh = Vec::with_capacity(SAMPLE_CAP);
    let mut v = samples.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::replace(&mut *v, fresh)
}

impl UsageCounters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_decide(&self, d: Duration) {
        push(&self.decide_all_us, d);
    }

    pub fn record_spellcheck(&self, d: Duration) {
        push(&self.spellcheck_us, d);
    }

    /// Take everything recorded since the last call. Allocates the next buffers here, off the
    /// hook thread.
    pub fn take(&self) -> UsageTaken {
        UsageTaken {
            active_minutes: self.active_minutes.swap(0, Ordering::Relaxed),
            skipped_secure: self.skipped_secure.swap(0, Ordering::Relaxed),
            decide_all_us: take_samples(&self.decide_all_us),
            spellcheck_us: take_samples(&self.spellcheck_us),
        }
    }
}

/// Counts distinct minutes of typing from a millisecond tick: true the first time a key arrives
/// in a new minute.
#[derive(Debug, Default, Clone, Copy)]
pub struct MinuteMarks {
    last: Option<u32>,
}

impl MinuteMarks {
    pub fn new_minute(&mut self, tick_ms: u32) -> bool {
        let minute = tick_ms / 60_000;
        self.last.replace(minute) != Some(minute)
    }
}

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
    fn usage_samples_are_taken_once_and_capped() {
        let c = UsageCounters::new();
        c.record_decide(Duration::from_micros(90));
        c.record_spellcheck(Duration::from_micros(40));
        c.active_minutes.fetch_add(2, Ordering::Relaxed);
        let t = c.take();
        assert_eq!(t.decide_all_us, [90]);
        assert_eq!(t.spellcheck_us, [40]);
        assert_eq!(t.active_minutes, 2);
        assert_eq!(c.take(), UsageTaken::default());
        for _ in 0..SAMPLE_CAP + 10 {
            c.record_decide(Duration::from_micros(1));
        }
        assert_eq!(c.take().decide_all_us.len(), SAMPLE_CAP);
    }

    #[test]
    fn minutes_count_once_each() {
        let mut m = MinuteMarks::default();
        assert!(m.new_minute(1_000));
        assert!(!m.new_minute(59_999));
        assert!(m.new_minute(60_000));
        assert!(!m.new_minute(119_000));
        assert!(m.new_minute(300_000));
    }

    #[test]
    fn small_values_are_exact() {
        let mut h = LatencyHistogram::new();
        h.record(Duration::from_nanos(42));
        assert_eq!(h.percentile(0.5), Some(Duration::from_nanos(42)));
    }
}
