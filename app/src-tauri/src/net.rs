//! What requests carry, and the HTTP client for the calls the updater doesn't make itself (the
//! manifest, usage stats).
//!
//! There is no install ID and nothing identifying in any request. The manifest and the release
//! files on dl.rmgn.net are static downloads with only `User-Agent: unflub/{version}`; the
//! rollout bucket from 0 to 99 (drawn at random for each installed version, so it links nothing
//! from one release to the next) never leaves the computer: the app compares it with the
//! manifest's rollout itself. Usage stats to api.rmgn.net carry no identity headers at all.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::{fs, path::PathBuf, time::Duration};

pub type Headers = Vec<(HeaderName, HeaderValue)>;

/// `{target}` in the manifest's platform keys and the release files: the OS family.
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

/// What every request to dl.rmgn.net carries: `User-Agent: unflub/{version}`, nothing else.
pub fn headers(version: &str) -> Headers {
    HeaderValue::from_str(&user_agent(version))
        .map(|v| vec![(USER_AGENT, v)])
        .unwrap_or_default()
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
    fn headers_carry_only_the_version() {
        let headers = headers("0.2.0");
        assert_eq!(get(&headers, "user-agent").as_deref(), Some("unflub/0.2.0"));
        assert_eq!(headers.len(), 1);
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
