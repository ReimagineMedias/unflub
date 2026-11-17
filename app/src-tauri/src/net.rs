//! What requests to api.rmgn.ca carry, and the HTTP client for the calls the updater doesn't
//! make itself (remote config, usage stats). Contract: `D:\Code\rmgn-api\docs\unflub-contract.md`.
//!
//! There is no install ID. The update check and remote config carry the app and OS versions, a
//! rollout bucket from 0 to 99 drawn at random for each installed version (so it links nothing
//! from one release to the next), and `X-First`: which UTC periods (day, ISO week, month) this is
//! the first update check in. Usage stats carry no identity headers at all.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::{fs, path::PathBuf, time::Duration};

pub const OS_VERSION_HEADER: &str = "X-OS-Version";
pub const ROLLOUT_BUCKET_HEADER: &str = "X-Rollout-Bucket";
pub const FIRST_HEADER: &str = "X-First";

pub type Headers = Vec<(HeaderName, HeaderValue)>;

/// `{{target}}` in the contract: the OS family.
pub fn target() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

pub fn user_agent(version: &str) -> String {
    format!("unflub/{version}")
}

/// A random number below `n`, from the OS's random source (through `uuid`).
fn random_below(n: u32) -> u32 {
    let b = uuid::Uuid::new_v4().into_bytes();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) % n
}

/// The rollout bucket: a random 0..=99 drawn once per installed version and kept in
/// `rollout-bucket` (`<version> <bucket>`) until the version changes.
pub struct Rollout {
    path: PathBuf,
}

impl Rollout {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("rollout-bucket"),
        }
    }

    pub fn bucket(&self, version: &str) -> u8 {
        let saved = fs::read_to_string(&self.path).ok().and_then(|text| {
            let (v, b) = text.trim().split_once(' ')?;
            let b: u8 = b.parse().ok()?;
            (v == version && b < 100).then_some(b)
        });
        if let Some(b) = saved {
            return b;
        }
        let b = random_below(100) as u8;
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&self.path, format!("{version} {b}"));
        b
    }
}

/// Which UTC periods `now` starts compared with the last update check at `last` (Unix
/// seconds): `day`, `week` (ISO week, Monday 00:00 UTC) and `month`. All three when there was
/// none.
pub fn first_periods(last: Option<u64>, now: u64) -> Vec<&'static str> {
    let date = |secs: u64| {
        time::OffsetDateTime::from_unix_timestamp(secs as i64)
            .map(|t| t.date())
            .ok()
    };
    let (Some(last), Some(today)) = (last.and_then(date), date(now)) else {
        return vec!["day", "week", "month"];
    };
    let mut first = Vec::with_capacity(3);
    if today != last {
        first.push("day");
    }
    let (y1, w1, _) = last.to_iso_week_date();
    let (y2, w2, _) = today.to_iso_week_date();
    if (y1, w1) != (y2, w2) {
        first.push("week");
    }
    if (last.year(), last.month()) != (today.year(), today.month()) {
        first.push("month");
    }
    first
}

/// `User-Agent` and `X-OS-Version`.
pub fn base_headers(version: &str, os_version: &str) -> Headers {
    let mut headers = Vec::with_capacity(4);
    if let Ok(v) = HeaderValue::from_str(&user_agent(version)) {
        headers.push((USER_AGENT, v));
    }
    if let Ok(v) = HeaderValue::from_str(os_version) {
        headers.push((HeaderName::from_static("x-os-version"), v));
    }
    headers
}

/// The update check's and remote config's headers: the base ones, the rollout bucket, and
/// `X-First` (left out when empty).
pub fn update_headers(version: &str, os_version: &str, bucket: u8, first: &[&str]) -> Headers {
    let mut headers = base_headers(version, os_version);
    headers.push((
        HeaderName::from_static("x-rollout-bucket"),
        HeaderValue::from(u16::from(bucket.min(99))),
    ));
    if !first.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&first.join(",")) {
            headers.push((HeaderName::from_static("x-first"), v));
        }
    }
    headers
}

/// A client with `headers` on every request. It refuses plain-http URLs and redirects.
pub fn client(headers: Headers) -> reqwest::Result<reqwest::Client> {
    // The updater installs the same provider before its first request; whichever runs first wins.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .default_headers(headers.into_iter().collect::<HeaderMap>())
        .https_only(true)
        .timeout(Duration::from_secs(30))
        .build()
}

/// A random duration below `max`, for spreading requests out.
pub fn jitter(max: Duration) -> Duration {
    let max_ms = u32::try_from(max.as_millis()).unwrap_or(u32::MAX).max(1);
    Duration::from_millis(u64::from(random_below(max_ms)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(headers: &Headers, name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(k, _)| k.as_str().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.to_str().unwrap().to_string())
    }

    #[test]
    fn update_headers_follow_the_contract() {
        let headers = update_headers("0.2.0", "10.0.26300", 42, &["day", "month"]);
        assert_eq!(get(&headers, "user-agent").as_deref(), Some("unflub/0.2.0"));
        assert_eq!(
            get(&headers, OS_VERSION_HEADER).as_deref(),
            Some("10.0.26300")
        );
        assert_eq!(get(&headers, ROLLOUT_BUCKET_HEADER).as_deref(), Some("42"));
        assert_eq!(get(&headers, FIRST_HEADER).as_deref(), Some("day,month"));
        assert_eq!(headers.len(), 4);
        // Nothing that could tell installs apart.
        assert!(headers.iter().all(|(k, _)| !k.as_str().contains("install")));
        let quiet = update_headers("0.2.0", "10.0.26300", 0, &[]);
        assert_eq!(get(&quiet, FIRST_HEADER), None);
    }

    #[test]
    fn bucket_is_kept_per_version_and_redrawn_on_upgrade() {
        let dir = std::env::temp_dir().join(format!("unflub-bucket-{}", std::process::id()));
        let rollout = Rollout::new(dir.clone());
        let b = rollout.bucket("0.2.0");
        assert!(b < 100);
        for _ in 0..5 {
            assert_eq!(rollout.bucket("0.2.0"), b, "stable within a version");
        }
        // A new version draws again: over 30 upgrades at least two buckets differ.
        let seen: std::collections::BTreeSet<u8> = (0..30)
            .map(|i| rollout.bucket(&format!("0.{i}.0")))
            .collect();
        assert!(seen.len() > 1);
        // A broken file is redrawn.
        fs::write(dir.join("rollout-bucket"), "0.2.0 250").unwrap();
        assert!(rollout.bucket("0.2.0") < 100);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn first_periods_by_utc_calendar() {
        // 2026-11-17 (a Tuesday) 12:00 UTC.
        let tue = 1_794_916_800;
        let h = 3600;
        assert_eq!(first_periods(None, tue), ["day", "week", "month"]);
        assert!(first_periods(Some(tue - h), tue).is_empty(), "same day");
        assert!(
            first_periods(Some(tue - 12 * h), tue).is_empty(),
            "midnight UTC"
        );
        assert_eq!(
            first_periods(Some(tue - 12 * h - 1), tue),
            ["day"],
            "Monday 23:59:59"
        );
        assert_eq!(
            first_periods(Some(tue - 3 * 24 * h), tue),
            ["day", "week"],
            "last Saturday"
        );
        assert_eq!(
            first_periods(Some(tue - 20 * 24 * h), tue),
            ["day", "week", "month"]
        );
    }

    /// Refused before any connection is made.
    #[test]
    fn plain_http_is_refused() {
        let client = client(Vec::new()).unwrap();
        let err = tauri::async_runtime::block_on(client.get("http://unflub.invalid/x").send())
            .unwrap_err();
        assert!(err.is_builder(), "{err}");
    }

    #[test]
    fn jitter_stays_in_range() {
        for _ in 0..100 {
            assert!(jitter(Duration::from_secs(60)) < Duration::from_secs(60));
        }
        assert_eq!(jitter(Duration::ZERO), Duration::ZERO);
    }
}
