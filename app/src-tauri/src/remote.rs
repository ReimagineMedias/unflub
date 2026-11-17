//! Remote config: feature flags and an optional message from api.rmgn.ca, so a misbehaving
//! feature can be turned off without a release.
//!
//! Fetched at startup and with every update check, cached on disk with its ETag, and read from
//! the cache when offline. Anything missing, unknown or malformed falls back to
//! [`RemoteFlags::default`], which is how the app behaves with no server at all.
//!
//! **Invariant: remote config only takes away.** Every flag can switch a feature off or make a
//! limit stricter; none can switch on something the user (or a machine policy) turned off, and
//! the defaults are the most permissive values. Each consumer ANDs a flag with its own setting.
//! The tests below check this for every combination.

use crate::{links, net};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Mutex};

/// Emitted with a [`RemoteMessage`] (or `null`) when the message to show changes.
pub const MESSAGE_EVENT: &str = "remote-message";

/// `GET /unflub/v1/config` response body.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub flags: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub message: Option<RemoteMessage>,
}

/// A short note for the About page, like "0.3.1 fixes a crash in Slack. Update when you can."
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
    /// Accept opt-in usage stats. Off pauses sending (counters stay local).
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
    pub fn from_config(config: &RemoteConfig) -> Self {
        let d = Self::default();
        let flag_bool = |name: &str, default: bool| {
            config
                .flags
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        Self {
            pill_enabled: flag_bool("pill_enabled", d.pill_enabled),
            uia_caret: flag_bool("uia_caret", d.uia_caret),
            stats_enabled: flag_bool("stats_enabled", d.stats_enabled),
            min_check_interval_h: config
                .flags
                .get("min_check_interval_h")
                .and_then(serde_json::Value::as_u64)
                .map_or(d.min_check_interval_h, |h| h.clamp(1, 168) as u32),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Cache {
    etag: Option<String>,
    config: RemoteConfig,
    /// Messages the user closed.
    #[serde(default)]
    dismissed: Vec<String>,
}

pub struct Remote {
    path: PathBuf,
    cache: Mutex<Cache>,
    /// The `DisableRemoteConfig` policy: defaults only, nothing fetched or shown.
    disabled: bool,
}

impl Remote {
    /// Reads the cached config, if any.
    pub fn new(dir: PathBuf, disabled: bool) -> Self {
        let path = dir.join("remote-config.json");
        let cache = if disabled {
            Cache::default()
        } else {
            fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        };
        Self {
            path,
            cache: Mutex::new(cache),
            disabled,
        }
    }

    pub fn flags(&self) -> RemoteFlags {
        if self.disabled {
            return RemoteFlags::default();
        }
        RemoteFlags::from_config(&self.cache.lock().unwrap().config)
    }

    /// The message to show, unless the user closed it. Its link is dropped unless it is one of
    /// ours ([`links::openable`]).
    pub fn message(&self) -> Option<RemoteMessage> {
        if self.disabled {
            return None;
        }
        let cache = self.cache.lock().unwrap();
        let mut message = cache.config.message.clone()?;
        if cache.dismissed.contains(&message.id) {
            return None;
        }
        message.url = message.url.filter(|u| links::openable(u));
        Some(message)
    }

    pub fn dismiss(&self, id: &str) {
        let mut cache = self.cache.lock().unwrap();
        if !cache.dismissed.iter().any(|d| d == id) {
            cache.dismissed.push(id.to_string());
            // Old ids are useless once the server stops sending them; keep the list short.
            let len = cache.dismissed.len();
            cache.dismissed.drain(..len.saturating_sub(20));
        }
        self.save(&cache);
    }

    fn save(&self, cache: &Cache) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = serde_json::to_vec_pretty(cache)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&tmp, bytes))
            .and_then(|()| fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            eprintln!("remote config: can't cache it ({e})");
        }
    }

    /// Fetch the config. Returns true when it changed. Errors keep the cached copy.
    pub async fn refresh(&self, client: &reqwest::Client, channel: &str, version: &str) -> bool {
        let etag = self.cache.lock().unwrap().etag.clone();
        let url = config_url(version, channel);
        let mut request = client.get(url).header(header::ACCEPT, "application/json");
        if let Some(etag) = etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("remote config: {}", e.without_url());
                return false;
            }
        };
        if response.status() == StatusCode::NOT_MODIFIED || !response.status().is_success() {
            return false;
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let config = match response.json::<RemoteConfig>().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("remote config: unreadable ({})", e.without_url());
                return false;
            }
        };
        let mut cache = self.cache.lock().unwrap();
        let changed = cache.config != config;
        cache.etag = etag;
        cache.config = config;
        self.save(&cache);
        changed
    }
}

fn config_url(version: &str, channel: &str) -> String {
    format!(
        "{}/config?version={version}&target={}&channel={channel}",
        links::API_BASE,
        net::target()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> RemoteConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn defaults_without_a_server() {
        let flags = RemoteFlags::from_config(&RemoteConfig::default());
        assert_eq!(flags, RemoteFlags::default());
        assert!(flags.pill_enabled && flags.uia_caret && flags.stats_enabled);
        assert_eq!(flags.min_check_interval_h, 1);
    }

    #[test]
    fn reads_known_flags_and_ignores_the_rest() {
        let c = config(
            r#"{ "schema": 1, "flags": { "pill_enabled": false, "uia_caret": "no",
                 "min_check_interval_h": 6, "brand_new_flag": 3 }, "message": null }"#,
        );
        let flags = RemoteFlags::from_config(&c);
        assert!(!flags.pill_enabled);
        assert!(flags.uia_caret, "a wrong type keeps the default");
        assert!(flags.stats_enabled);
        assert_eq!(flags.min_check_interval_h, 6);
    }

    #[test]
    fn clamps_the_check_interval() {
        for (h, want) in [(0, 1), (1000, 168), (24, 24)] {
            let c = config(&format!(
                r#"{{ "flags": {{ "min_check_interval_h": {h} }} }}"#
            ));
            assert_eq!(RemoteFlags::from_config(&c).min_check_interval_h, want);
        }
    }

    #[test]
    fn messages_only_link_to_our_sites_and_can_be_dismissed() {
        let dir = std::env::temp_dir().join(format!("unflub-remote-{}", std::process::id()));
        let remote = Remote::new(dir.clone(), false);
        remote.cache.lock().unwrap().config = config(
            r#"{ "schema": 1, "flags": {}, "message": { "id": "m1", "text": "Hi", "url": "https://evil.example/" } }"#,
        );
        let m = remote.message().unwrap();
        assert_eq!(m.text, "Hi");
        assert_eq!(m.url, None);
        remote.dismiss("m1");
        assert_eq!(remote.message(), None);
        // Cached on disk, dismissal included.
        assert_eq!(Remote::new(dir.clone(), false).message(), None);
        let _ = fs::remove_dir_all(dir);
    }

    /// Every possible config yields flags no more permissive than the defaults, so ANDing them
    /// with a user setting can only turn things off.
    #[test]
    fn remote_config_only_takes_away() {
        let d = RemoteFlags::default();
        let bools = ["true", "false", "\"yes\"", "1", "null"];
        let hours = ["0", "1", "6", "1000", "-5", "\"24\"", "2.5"];
        for pill in bools {
            for uia in bools {
                for stats in bools {
                    for h in hours {
                        let c = config(&format!(
                            r#"{{ "flags": {{ "pill_enabled": {pill}, "uia_caret": {uia},
                                 "stats_enabled": {stats}, "min_check_interval_h": {h} }} }}"#
                        ));
                        let f = RemoteFlags::from_config(&c);
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
    fn policy_disables_remote_config() {
        let dir = std::env::temp_dir().join(format!("unflub-remote-off-{}", std::process::id()));
        let remote = Remote::new(dir, true);
        remote.cache.lock().unwrap().config = config(
            r#"{ "flags": { "pill_enabled": false }, "message": { "id": "m", "text": "x" } }"#,
        );
        assert_eq!(remote.flags(), RemoteFlags::default());
        assert_eq!(remote.message(), None);
    }

    #[test]
    fn url_follows_the_contract() {
        assert_eq!(
            config_url("0.2.0", "beta"),
            format!(
                "https://api.rmgn.ca/unflub/v1/config?version=0.2.0&target={}&channel=beta",
                net::target()
            )
        );
    }
}
