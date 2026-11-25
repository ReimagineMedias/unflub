//! Remote flags and the in-app message, from the signed manifest (`manifest.rs`), so a
//! misbehaving feature can be turned off without a release.
//!
//! The manifest for the current channel is fetched at startup and with every update check,
//! with `If-None-Match`, and cached on disk next to its signature and ETag. The cached copy is
//! verified again when it is read, so an edited file counts as no manifest. Anything missing,
//! unknown or malformed falls back to [`RemoteFlags::default`], which is how the app behaves
//! with no server at all.
//!
//! **Invariant: remote flags only take away.** Every flag can switch a feature off or make a
//! limit stricter; none can switch on something the user (or a machine policy) turned off, and
//! the defaults are the most permissive values. Each consumer ANDs a flag with its own setting.
//! The tests below check this for every combination.

use crate::{
    keys::{ManifestKey, MANIFEST_KEYS},
    links,
    manifest::{self, Manifest, Rejected},
};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Mutex};

/// Emitted with a [`RemoteMessage`] (or `null`) when the message to show changes.
pub const MESSAGE_EVENT: &str = "remote-message";

/// A short note for the About page and the tray, like "0.3.1 fixes a crash in Slack. Update
/// when you can."
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteMessage {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// The flags the app acts on. Each default is the normal behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFlags {
    /// Show the correction pill at all.
    pub pill_enabled: bool,
    /// Ask UI Automation for the caret in browser and XAML fields (the pill's only cost to
    /// other apps).
    pub uia_caret: bool,
    /// Accept usage stats. Off pauses sending: each finished day is dropped unsent.
    pub stats_enabled: bool,
    /// Fewest hours between two update checks, 1 to 168.
    pub min_check_interval_h: u32,
}

impl Default for RemoteFlags {
    fn default() -> Self {
        Self {
            pill_enabled: true,
            uia_caret: true,
            stats_enabled: true,
            min_check_interval_h: 1,
        }
    }
}

impl RemoteFlags {
    /// Known flags with the right type are applied; everything else is ignored.
    pub fn from_flags(flags: &BTreeMap<String, serde_json::Value>) -> Self {
        let d = Self::default();
        let flag_bool = |name: &str, default: bool| {
            flags
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        Self {
            pill_enabled: flag_bool("pill_enabled", d.pill_enabled),
            uia_caret: flag_bool("uia_caret", d.uia_caret),
            stats_enabled: flag_bool("stats_enabled", d.stats_enabled),
            min_check_interval_h: flags
                .get("min_check_interval_h")
                .and_then(serde_json::Value::as_u64)
                .map_or(d.min_check_interval_h, |h| h.clamp(1, 168) as u32),
        }
    }
}

/// Why a refresh didn't produce a new manifest.
#[derive(Debug)]
pub enum RefreshError {
    /// Offline, or the server answered with an error.
    Network(String),
    /// A manifest arrived but wasn't believed; the last good one stays.
    Rejected(Rejected),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "{e}"),
            Self::Rejected(r) => write!(f, "{r}"),
        }
    }
}

/// `state.json` beside the cached manifests.
#[derive(Debug, Default, Deserialize, Serialize)]
struct Meta {
    /// ETag per channel.
    #[serde(default)]
    etags: BTreeMap<String, String>,
    /// Messages the user closed.
    #[serde(default)]
    dismissed: Vec<String>,
}

#[derive(Default)]
struct State {
    /// The channel `manifest` belongs to.
    channel: String,
    manifest: Option<Manifest>,
    meta: Meta,
}

pub struct Remote {
    dir: PathBuf,
    keys: &'static [ManifestKey],
    state: Mutex<State>,
    /// The `DisableRemoteConfig` policy: built-in flags only, no message. The manifest is
    /// still used for updates unless `DisableUpdates` is set too.
    disabled: bool,
    /// This app's version and rollout bucket, for flag targeting ([`manifest::resolve_flags`]).
    version: semver::Version,
    bucket: u8,
}

impl Remote {
    /// Reads the cached manifest for `channel`, if any.
    pub fn new(
        data_dir: PathBuf,
        disabled: bool,
        channel: &str,
        version: semver::Version,
        bucket: u8,
    ) -> Self {
        let mut remote =
            Self::with_keys(data_dir.join("manifest"), disabled, channel, MANIFEST_KEYS);
        remote.version = version;
        remote.bucket = bucket;
        remote
    }

    fn with_keys(
        dir: PathBuf,
        disabled: bool,
        channel: &str,
        keys: &'static [ManifestKey],
    ) -> Self {
        let meta = fs::read_to_string(dir.join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let remote = Self {
            dir,
            keys,
            state: Mutex::new(State {
                meta,
                ..State::default()
            }),
            disabled,
            version: semver::Version::new(0, 0, 0),
            bucket: 99,
        };
        remote.load(&mut remote.state.lock().unwrap(), channel);
        remote
    }

    fn body_path(&self, channel: &str) -> PathBuf {
        self.dir.join(format!("{channel}.json"))
    }

    fn sig_path(&self, channel: &str) -> PathBuf {
        self.dir.join(format!("{channel}.json.sig"))
    }

    /// Switch `state` to `channel`'s cached manifest, verified again.
    fn load(&self, state: &mut State, channel: &str) {
        if state.channel == channel {
            return;
        }
        state.channel = channel.to_string();
        state.manifest = match (
            fs::read(self.body_path(channel)),
            fs::read(self.sig_path(channel)),
        ) {
            (Ok(body), Ok(sig)) => manifest::verify(&body, &sig, self.keys)
                .ok()
                .filter(|m| m.channel == channel),
            _ => None,
        };
        if state.manifest.is_none() {
            state.meta.etags.remove(channel);
        }
    }

    pub fn flags(&self) -> RemoteFlags {
        if self.disabled {
            return RemoteFlags::default();
        }
        let state = self.state.lock().unwrap();
        state
            .manifest
            .as_ref()
            .map_or_else(RemoteFlags::default, |m| {
                RemoteFlags::from_flags(&manifest::resolve_flags(
                    m,
                    &self.version,
                    crate::net::target(),
                    self.bucket,
                ))
            })
    }

    /// The message to show, unless the user closed it. Its link is dropped unless it is one of
    /// ours ([`links::openable`]).
    pub fn message(&self) -> Option<RemoteMessage> {
        if self.disabled {
            return None;
        }
        let state = self.state.lock().unwrap();
        let mut message = state.manifest.as_ref()?.message.clone()?;
        if state.meta.dismissed.contains(&message.id) {
            return None;
        }
        message.url = message.url.filter(|u| links::openable(u));
        Some(message)
    }

    pub fn dismiss(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        if !state.meta.dismissed.iter().any(|d| d == id) {
            state.meta.dismissed.push(id.to_string());
            // Old ids are useless once the manifest stops carrying them; keep the list short.
            let len = state.meta.dismissed.len();
            state.meta.dismissed.drain(..len.saturating_sub(20));
        }
        self.save_meta(&state.meta);
    }

    /// The last good manifest for `channel`, for the update check.
    pub fn manifest(&self, channel: &str) -> Option<Manifest> {
        let mut state = self.state.lock().unwrap();
        self.load(&mut state, channel);
        state.manifest.clone()
    }

    /// The ETag to send, only while a verified manifest for `channel` is cached.
    fn etag(&self, channel: &str) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        self.load(&mut state, channel);
        state.manifest.as_ref()?;
        state.meta.etags.get(channel).cloned()
    }

    fn write(&self, name: PathBuf, bytes: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = name.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &name)
    }

    fn save_meta(&self, meta: &Meta) {
        let written = serde_json::to_vec_pretty(meta)
            .map_err(std::io::Error::other)
            .and_then(|bytes| self.write(self.dir.join("state.json"), &bytes));
        if let Err(e) = written {
            eprintln!("manifest: can't save its state ({e})");
        }
    }

    /// Take a downloaded manifest: verify it, refuse one for another channel or older than the
    /// cached one, then cache it with its signature and ETag. True when the flags or the
    /// message changed.
    fn accept(
        &self,
        channel: &str,
        body: &[u8],
        sig: &[u8],
        etag: Option<String>,
    ) -> Result<bool, Rejected> {
        let next = manifest::verify(body, sig, self.keys)?;
        if next.channel != channel {
            return Err(Rejected::WrongChannel);
        }
        if next.expired(time::OffsetDateTime::now_utc()) {
            return Err(Rejected::Expired);
        }
        let mut state = self.state.lock().unwrap();
        self.load(&mut state, channel);
        manifest::replaces(&next, state.manifest.as_ref())?;
        let changed = state
            .manifest
            .as_ref()
            .is_none_or(|old| old.flags != next.flags || old.message != next.message);
        let saved = self
            .write(self.body_path(channel), body)
            .and_then(|()| self.write(self.sig_path(channel), sig));
        if let Err(e) = saved {
            eprintln!("manifest: can't cache it ({e})");
        }
        match etag {
            Some(etag) => state.meta.etags.insert(channel.to_string(), etag),
            None => state.meta.etags.remove(channel),
        };
        state.manifest = Some(next);
        self.save_meta(&state.meta);
        Ok(changed)
    }

    /// Fetch the manifest and its signature. `Ok(true)` when the flags or the message changed.
    /// Any error keeps the last good manifest.
    pub async fn refresh(
        &self,
        client: &reqwest::Client,
        channel: &str,
    ) -> Result<bool, RefreshError> {
        let etag = self.etag(channel);
        let Some((body, sig, etag)) = fetch(client, channel, etag.as_deref(), None).await? else {
            return Ok(false);
        };
        match self.accept(channel, &body, &sig, etag) {
            // The two files are cached apart and can disagree for a few minutes after a
            // change: try once more past the cache (contract section 1), then keep the last
            // accepted manifest.
            Err(Rejected::BadSignature | Rejected::Unsigned) => {
                let minute = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() / 60);
                match fetch(client, channel, None, Some(minute)).await? {
                    Some((body, sig, etag)) => self
                        .accept(channel, &body, &sig, etag)
                        .map_err(RefreshError::Rejected),
                    None => Ok(false),
                }
            }
            other => other.map_err(RefreshError::Rejected),
        }
    }
}

/// The manifest and its signature, or `None` when the manifest is unchanged (304). `bust` adds
/// `?t=<unix minute>` to both URLs.
async fn fetch(
    client: &reqwest::Client,
    channel: &str,
    etag: Option<&str>,
    bust: Option<u64>,
) -> Result<Option<(Vec<u8>, Vec<u8>, Option<String>)>, RefreshError> {
    let net = |e: reqwest::Error| RefreshError::Network(e.without_url().to_string());
    let response = manifest_request(client, channel, etag, bust)
        .send()
        .await
        .map_err(net)?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(RefreshError::Network(format!(
            "the update server answered {}",
            response.status()
        )));
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body = read_limited(response, manifest::MAX_BYTES).await?;
    let sig = client
        .get(with_bust(manifest::signature_url(channel), bust))
        .send()
        .await
        .map_err(net)?;
    let sig = match sig.status() {
        s if s.is_success() => read_limited(sig, manifest::MAX_SIG_BYTES).await?,
        StatusCode::NOT_FOUND => Vec::new(),
        s => {
            return Err(RefreshError::Network(format!(
                "the update server answered {s}"
            )))
        }
    };
    Ok(Some((body, sig, etag)))
}

fn with_bust(url: String, bust: Option<u64>) -> String {
    match bust {
        Some(t) => format!("{url}?t={t}"),
        None => url,
    }
}

/// `GET {channel}.json`, conditional on the cached ETag.
fn manifest_request(
    client: &reqwest::Client,
    channel: &str,
    etag: Option<&str>,
    bust: Option<u64>,
) -> reqwest::RequestBuilder {
    let mut request = client
        .get(with_bust(manifest::manifest_url(channel), bust))
        .header(header::ACCEPT, "application/json");
    if let Some(etag) = etag {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    request
}

/// The body, refused as soon as it passes `max` bytes.
async fn read_limited(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, RefreshError> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(RefreshError::Rejected(Rejected::TooLarge));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| RefreshError::Network(e.without_url().to_string()))?
    {
        if body.len() + chunk.len() > max {
            return Err(RefreshError::Rejected(Rejected::TooLarge));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{manifest_json, sign, test_keys};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("unflub-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn remote(dir: &std::path::Path, disabled: bool) -> Remote {
        let mut r = Remote::with_keys(dir.to_path_buf(), disabled, "stable", test_keys());
        r.version = semver::Version::new(0, 1, 0);
        r.bucket = 0;
        r
    }

    /// `{"name": {value, targeting for everyone}}`, as the API writes flags.
    fn flag(name: &str, value: &str) -> String {
        format!(
            r#"{{"{name}":{{"value":{value},"min_version":null,"max_version":null,"target":null,"rollout_percent":100,"bucket_offset":0}}}}"#
        )
    }

    fn flags(json: &str) -> RemoteFlags {
        RemoteFlags::from_flags(&serde_json::from_str(json).unwrap())
    }

    /// Accept a manifest generated at `at` with `extra` fields (`"flags":…` replaces the empty
    /// one).
    fn give(r: &Remote, at: &str, extra: &str, etag: Option<&str>) -> Result<bool, Rejected> {
        let body = manifest_json(at, "");
        let body = if extra.is_empty() {
            body
        } else {
            body.replace("\"flags\":{},\"message\":null", extra)
        };
        r.accept(
            "stable",
            body.as_bytes(),
            &sign(body.as_bytes()),
            etag.map(String::from),
        )
    }

    #[test]
    fn defaults_without_a_server() {
        let dir = temp("remote-none");
        let r = remote(&dir, false);
        assert_eq!(r.flags(), RemoteFlags::default());
        let d = RemoteFlags::default();
        assert!(d.pill_enabled && d.uia_caret && d.stats_enabled);
        assert_eq!(d.min_check_interval_h, 1);
        assert_eq!(r.message(), None);
        assert_eq!(r.manifest("stable"), None);
    }

    #[test]
    fn reads_known_flags_and_ignores_the_rest() {
        let f = flags(
            r#"{ "pill_enabled": false, "uia_caret": "no", "min_check_interval_h": 6, "brand_new_flag": 3 }"#,
        );
        assert!(!f.pill_enabled);
        assert!(f.uia_caret, "a wrong type keeps the default");
        assert!(f.stats_enabled);
        assert_eq!(f.min_check_interval_h, 6);
    }

    #[test]
    fn clamps_the_check_interval() {
        for (h, want) in [(0, 1), (1000, 168), (24, 24)] {
            let f = flags(&format!(r#"{{ "min_check_interval_h": {h} }}"#));
            assert_eq!(f.min_check_interval_h, want);
        }
    }

    #[test]
    fn flags_come_from_a_verified_manifest_and_survive_a_restart() {
        let dir = temp("remote-flags");
        let r = remote(&dir, false);
        assert_eq!(
            give(
                &r,
                "2026-11-17T12:00:00Z",
                &format!(
                    r#""flags":{},"message":null"#,
                    flag("pill_enabled", "false")
                ),
                None
            ),
            Ok(true)
        );
        assert!(!r.flags().pill_enabled);
        assert!(!remote(&dir, false).flags().pill_enabled, "cached on disk");
        // An edited cache file is not believed.
        let body = dir.join("stable.json");
        let text = fs::read_to_string(&body).unwrap();
        fs::write(&body, text.replace("false", "true ")).unwrap();
        assert_eq!(remote(&dir, false).manifest("stable"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejected_manifests_keep_the_last_good_one() {
        let dir = temp("remote-reject");
        let r = remote(&dir, false);
        give(
            &r,
            "2026-11-17T12:00:00Z",
            &format!(
                r#""flags":{},"message":null"#,
                flag("stats_enabled", "false")
            ),
            None,
        )
        .unwrap();
        // Badly signed.
        let body = manifest_json("2026-11-18T12:00:00Z", "");
        assert_eq!(
            r.accept("stable", body.as_bytes(), b"", None),
            Err(Rejected::Unsigned)
        );
        let mut sig = sign(body.as_bytes());
        sig[3] ^= 1;
        assert!(r.accept("stable", body.as_bytes(), &sig, None).is_err());
        // Signed, but older: a replay.
        assert_eq!(
            give(&r, "2026-11-17T11:00:00Z", "", None),
            Err(Rejected::Older)
        );
        // Signed, but for the other channel.
        let beta = body.replace("\"channel\":\"stable\"", "\"channel\":\"beta\"");
        assert_eq!(
            r.accept("stable", beta.as_bytes(), &sign(beta.as_bytes()), None),
            Err(Rejected::WrongChannel)
        );
        assert!(!r.flags().stats_enabled, "the last good manifest stands");
        // Rollback protection survives a restart.
        assert_eq!(
            give(&remote(&dir, false), "2026-11-17T11:00:00Z", "", None),
            Err(Rejected::Older)
        );
        // Newer and signed replaces it.
        assert_eq!(give(&r, "2026-11-17T13:00:00Z", "", None), Ok(true));
        assert!(r.flags().stats_enabled);
        // Signed and newer, but past its expiry.
        let stale = manifest_json("2026-11-18T12:00:00Z", "")
            .replace("2999-01-01T00:00:00Z", "2001-01-01T00:00:00Z");
        assert_eq!(
            r.accept("stable", stale.as_bytes(), &sign(stale.as_bytes()), None),
            Err(Rejected::Expired)
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Flags apply only to the versions, OS and buckets they target.
    #[test]
    fn targeted_flags_follow_version_and_bucket() {
        let dir = temp("remote-targeted");
        let r = remote(&dir, false);
        let ranged = r#"{"pill_enabled":{"value":false,"min_version":"0.2.0","max_version":null,"target":null,"rollout_percent":100,"bucket_offset":0},
            "uia_caret":{"value":false,"min_version":null,"max_version":null,"target":null,"rollout_percent":10,"bucket_offset":95}}"#;
        give(
            &r,
            "2026-11-17T12:00:00Z",
            &format!(r#""flags":{ranged},"message":null"#),
            None,
        )
        .unwrap();
        // 0.1.0 is below the pill flag's range; bucket 0 + 95 = 95 is outside 10%.
        assert_eq!(r.flags(), RemoteFlags::default());
        let mut r = remote(&dir, false);
        r.version = semver::Version::new(0, 2, 0);
        r.bucket = 7; // (7 + 95) % 100 = 2 < 10
        let f = r.flags();
        assert!(!f.pill_enabled && !f.uia_caret);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn etag_is_cached_and_sent() {
        let dir = temp("remote-etag");
        let r = remote(&dir, false);
        assert_eq!(r.etag("stable"), None, "nothing cached, nothing to send");
        give(&r, "2026-11-17T12:00:00Z", "", Some("\"abc\"")).unwrap();
        assert_eq!(r.etag("stable").as_deref(), Some("\"abc\""));
        let again = remote(&dir, false);
        assert_eq!(again.etag("stable").as_deref(), Some("\"abc\""));
        assert_eq!(again.etag("beta"), None, "per channel");
        // The same manifest again changes nothing the app acts on.
        assert_eq!(
            give(&again, "2026-11-17T12:00:00Z", "", Some("\"abc\"")),
            Ok(false)
        );

        let client = crate::net::client(Vec::new()).unwrap();
        let req = manifest_request(&client, "stable", again.etag("stable").as_deref(), None)
            .build()
            .unwrap();
        assert_eq!(req.url().as_str(), manifest::manifest_url("stable"));
        assert_eq!(req.headers()[header::IF_NONE_MATCH], "\"abc\"");
        let fresh = manifest_request(&client, "stable", None, None)
            .build()
            .unwrap();
        assert!(fresh.headers().get(header::IF_NONE_MATCH).is_none());
        // The retry after a signature mismatch goes past the cache.
        let busted = manifest_request(&client, "beta", None, Some(29_915_280))
            .build()
            .unwrap();
        assert_eq!(
            busted.url().as_str(),
            "https://dl.rmgn.net/unflub/manifest/beta.json?t=29915280"
        );
        assert_eq!(
            with_bust(manifest::signature_url("beta"), Some(1)),
            "https://dl.rmgn.net/unflub/manifest/beta.json.sig?t=1"
        );

        // Without its manifest, an ETag is never sent (a 304 would leave nothing to use).
        fs::remove_file(dir.join("stable.json")).unwrap();
        assert_eq!(remote(&dir, false).etag("stable"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn messages_only_link_to_our_sites_and_can_be_dismissed() {
        let dir = temp("remote-message");
        let r = remote(&dir, false);
        give(
            &r,
            "2026-11-17T12:00:00Z",
            r#""flags":{},"message":{"id":"m1","text":"Hi","url":"https://evil.example/"}"#,
            None,
        )
        .unwrap();
        let m = r.message().unwrap();
        assert_eq!(m.text, "Hi");
        assert_eq!(m.url, None);
        r.dismiss("m1");
        assert_eq!(r.message(), None);
        // Dismissal is kept on disk.
        assert_eq!(remote(&dir, false).message(), None);
        let _ = fs::remove_dir_all(dir);
    }

    /// Every possible flag value yields flags no more permissive than the defaults, so ANDing
    /// them with a user setting can only turn things off.
    #[test]
    fn remote_flags_only_take_away() {
        let d = RemoteFlags::default();
        let bools = ["true", "false", "\"yes\"", "1", "null"];
        let hours = ["0", "1", "6", "1000", "-5", "\"24\"", "2.5"];
        for pill in bools {
            for uia in bools {
                for stats in bools {
                    for h in hours {
                        let f = flags(&format!(
                            r#"{{ "pill_enabled": {pill}, "uia_caret": {uia},
                                 "stats_enabled": {stats}, "min_check_interval_h": {h} }}"#
                        ));
                        assert!(!f.pill_enabled || d.pill_enabled);
                        assert!(!f.uia_caret || d.uia_caret);
                        assert!(!f.stats_enabled || d.stats_enabled);
                        assert!(f.min_check_interval_h >= d.min_check_interval_h);
                        // Consumers: the user's off stays off whatever the flags say.
                        assert!(!crate::usage::may_send(false, f));
                        let pill_off = crate::settings::Settings {
                            pill_style: crate::settings::PillStyle::Off,
                            ..crate::settings::Settings::default()
                        };
                        let p = crate::engine::engine_settings(&pill_off, &f, Vec::new()).pill;
                        assert_eq!(p.style, unflub_engine::protocol::PillStyle::Off);
                    }
                }
            }
        }
    }

    #[test]
    fn policy_disables_flags_and_message() {
        let dir = temp("remote-off");
        let r = remote(&dir, true);
        give(
            &r,
            "2026-11-17T12:00:00Z",
            &format!(
                r#""flags":{},"message":{{"id":"m","text":"x"}}"#,
                flag("pill_enabled", "false")
            ),
            None,
        )
        .unwrap();
        assert_eq!(r.flags(), RemoteFlags::default());
        assert_eq!(r.message(), None);
        // The manifest itself is still there for updates.
        assert!(r.manifest("stable").is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
