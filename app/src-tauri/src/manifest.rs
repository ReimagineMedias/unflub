//! The signed update manifest: one static file per channel on dl.rmgn.net that carries the
//! release list, the rollout, the kill switch, the remote flags and the in-app message.
//!
//! - `GET https://dl.rmgn.net/unflub/manifest/{channel}.json` and `{channel}.json.sig`, a
//!   detached Ed25519 signature over the exact bytes (`keys.rs`). Unsigned, badly signed,
//!   malformed or oversized (> 256 KB) manifests are rejected and the last good one stays.
//! - A manifest older than the cached one (`generated_at`) is rejected too, so a replayed old
//!   file can't bring back a paused release or switch a flag back on. One past its `expires_at`
//!   is refused when downloaded, and a cached one that has expired offers no updates (its flags,
//!   which can only switch things off, still apply).
//! - The rules ([`eligible`], [`resolve_flags`]) mirror the API's reference,
//!   `D:\Code\rmgn-api\src\core\eligibility.ts`, and its tests.
//! - Eligibility runs here, with the local rollout bucket ([`eligible`]). The app then points
//!   the Tauri updater at that release's static file,
//!   `https://dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json`, and checks that its
//!   download URL and signature are the ones in the signed manifest.
//!
//! Nothing identifying is sent: the requests are plain downloads.

use crate::{
    keys::{self, ManifestKey},
    links,
    remote::RemoteMessage,
};
use ring::signature::{UnparsedPublicKey, ED25519};
use semver::{BuildMetadata, Version};
use serde::Deserialize;
use std::{collections::BTreeMap, fmt};

/// Largest manifest accepted.
pub const MAX_BYTES: usize = 256 * 1024;
/// Largest signature file accepted: 64 raw bytes, or their base64 with a little whitespace.
pub const MAX_SIG_BYTES: usize = 1024;
pub const SCHEMA: u32 = 2;
pub const PRODUCT: &str = "unflub";

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub channel: String,
    /// RFC 3339. Newer manifests replace older ones, never the other way round.
    pub generated_at: String,
    /// RFC 3339. After it the manifest offers no updates, so a frozen copy can't pin an app.
    pub expires_at: String,
    /// Which key signed it (`keys.rs`).
    pub key_id: String,
    /// The kill switch: false offers nothing to anyone.
    pub updates_enabled: bool,
    /// Anything below it gets the newest active release as a critical update.
    #[serde(default)]
    pub min_version: Option<String>,
    /// Versions that must not stay installed; they get the newest active release, even older.
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Newest first.
    #[serde(default)]
    pub releases: Vec<Release>,
    /// Targeted flags; [`resolve_flags`] picks the ones that apply to this app.
    #[serde(default)]
    pub flags: BTreeMap<String, Flag>,
    #[serde(default)]
    pub message: Option<RemoteMessage>,
}

/// One remote flag and who it applies to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Flag {
    pub value: serde_json::Value,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub max_version: Option<String>,
    /// `windows`, `darwin`, `linux`, or `None` for all.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default = "full")]
    pub rollout_percent: u32,
    /// Added to the bucket (mod 100) so two 10% flags don't land on the same apps.
    #[serde(default)]
    pub bucket_offset: u32,
}

fn full() -> u32 {
    100
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// 0 to 100: installs whose bucket is below it get the release.
    #[serde(default)]
    pub rollout_percent: u32,
    /// Keyed `{target}-{arch}`, like `windows-x86_64`.
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Platform {
    /// The installer the updater runs.
    pub url: String,
    /// Its updater (minisign) signature, as in the release's Tauri file.
    pub signature: String,
    /// Lowercase hex SHA-256 of the installer.
    pub sha256: String,
    pub size: u64,
}

/// Why a manifest was not believed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    TooLarge,
    Unsigned,
    BadSignature,
    Malformed(String),
    WrongProduct,
    WrongChannel,
    /// Older than the one already cached (a replay).
    Older,
    /// Past its `expires_at`.
    Expired,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "the release list is too large"),
            Self::Unsigned => write!(f, "the release list isn't signed"),
            Self::BadSignature => write!(f, "the release list's signature is invalid"),
            Self::Malformed(e) => write!(f, "the release list is unreadable ({e})"),
            Self::WrongProduct => write!(f, "the release list is for another product"),
            Self::WrongChannel => write!(f, "the release list is for another channel"),
            Self::Older => write!(f, "the release list is older than the one already here"),
            Self::Expired => write!(f, "the release list has expired"),
        }
    }
}

fn rfc3339(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

impl Manifest {
    pub fn generated_at(&self) -> Option<time::OffsetDateTime> {
        rfc3339(&self.generated_at)
    }

    /// Past `expires_at` (or without a readable one).
    pub fn expired(&self, now: time::OffsetDateTime) -> bool {
        rfc3339(&self.expires_at).is_none_or(|e| e <= now)
    }
}

/// A detached signature: 64 raw bytes, or base64 of them.
fn decode_signature(sig: &[u8]) -> Option<[u8; 64]> {
    if sig.len() == 64 {
        return sig.try_into().ok();
    }
    if sig.is_empty() || sig.len() > MAX_SIG_BYTES {
        return None;
    }
    use base64::Engine;
    let text = std::str::from_utf8(sig).ok()?.trim();
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?
        .try_into()
        .ok()
}

/// Check the signature over the exact bytes with each trusted key, then parse. The manifest's
/// `key_id` must name the key that verified it.
pub fn verify(body: &[u8], sig: &[u8], trusted: &[ManifestKey]) -> Result<Manifest, Rejected> {
    if body.len() > MAX_BYTES {
        return Err(Rejected::TooLarge);
    }
    if sig.iter().all(u8::is_ascii_whitespace) {
        return Err(Rejected::Unsigned);
    }
    let sig = decode_signature(sig).ok_or(Rejected::BadSignature)?;
    let signer = trusted
        .iter()
        .find(|k| {
            keys::decode(k.public).is_some_and(|public| {
                UnparsedPublicKey::new(&ED25519, public)
                    .verify(body, &sig)
                    .is_ok()
            })
        })
        .ok_or(Rejected::BadSignature)?;
    let manifest: Manifest =
        serde_json::from_slice(body).map_err(|e| Rejected::Malformed(e.to_string()))?;
    if manifest.schema != SCHEMA {
        return Err(Rejected::Malformed(format!("schema {}", manifest.schema)));
    }
    if manifest.product != PRODUCT {
        return Err(Rejected::WrongProduct);
    }
    if manifest.key_id != signer.id {
        return Err(Rejected::BadSignature);
    }
    if manifest.generated_at().is_none() {
        return Err(Rejected::Malformed("generated_at".into()));
    }
    if rfc3339(&manifest.expires_at).is_none() {
        return Err(Rejected::Malformed("expires_at".into()));
    }
    Ok(manifest)
}

/// `next` may replace `cached` only when it is not older (rollback protection).
pub fn replaces(next: &Manifest, cached: Option<&Manifest>) -> Result<(), Rejected> {
    match (cached.and_then(Manifest::generated_at), next.generated_at()) {
        (Some(old), Some(new)) if new < old => Err(Rejected::Older),
        (_, None) => Err(Rejected::Malformed("generated_at".into())),
        _ => Ok(()),
    }
}

/// What the manifest offers this install.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub release: Release,
    pub platform: Platform,
    /// The current version is blocked or below `min_version`.
    pub critical: bool,
}

/// `windows-x86_64`: how platforms are keyed in the manifest.
pub fn platform_key() -> String {
    format!("{}-{}", crate::net::target(), std::env::consts::ARCH)
}

/// Build metadata never decides anything (`0.3.0+abc` is blocked when `0.3.0` is).
fn core(v: &Version) -> Version {
    Version {
        build: BuildMetadata::EMPTY,
        ..v.clone()
    }
}

/// The release this install should move to, if any. Same rules as `chooseUpdate` in the API:
///
/// 1. The kill switch off: nothing.
/// 2. The current version blocked: the newest release for this platform with
///    `rollout_percent > 0` other than this one, whatever the bucket, even an older one
///    (a rollback). Below `min_version`: the newest such release newer than this one. Both are
///    critical. With none, fall through to 3.
/// 3. The newest release newer than current whose `rollout_percent` is above the bucket. 0%
///    reaches nobody, 100% everyone.
///
/// Blocked versions are never offered.
pub fn eligible(m: &Manifest, current: &Version, bucket: u8, platform: &str) -> Option<Offer> {
    if !m.updates_enabled {
        return None;
    }
    let current = core(current);
    let parse = |s: &str| Version::parse(s.trim()).ok().map(|v| core(&v));
    let blocked: Vec<Version> = m.blocked.iter().filter_map(|s| parse(s)).collect();
    let mut releases: Vec<(Version, &Release, &Platform)> = m
        .releases
        .iter()
        .filter_map(|r| {
            let v = parse(&r.version)?;
            let p = r.platforms.get(platform)?;
            (!blocked.contains(&v)).then_some((v, r, p))
        })
        .collect();
    releases.sort_by(|a, b| b.0.cmp(&a.0));
    let offer = |(_, r, p): &(Version, &Release, &Platform), critical| Offer {
        release: (*r).clone(),
        platform: (*p).clone(),
        critical,
    };
    let is_blocked = blocked.contains(&current);
    let below_min = m
        .min_version
        .as_deref()
        .and_then(parse)
        .is_some_and(|min| current < min);
    if is_blocked || below_min {
        let pick = releases.iter().find(|(v, r, _)| {
            r.rollout_percent > 0
                && if is_blocked {
                    *v != current
                } else {
                    *v > current
                }
        });
        if let Some(pick) = pick {
            return Some(offer(pick, true));
        }
    }
    releases
        .iter()
        .find(|(v, r, _)| *v > current && u32::from(bucket) < r.rollout_percent.min(100))
        .map(|r| offer(r, false))
}

/// The flag values that apply to this app, like `resolveFlags` in the API: the version within
/// `[min_version, max_version]`, the target matching (or unset), and
/// `(bucket + bucket_offset) % 100 < rollout_percent`. Flags that don't apply are left out, so
/// the app keeps its built-in default.
pub fn resolve_flags(
    m: &Manifest,
    version: &Version,
    target: &str,
    bucket: u8,
) -> BTreeMap<String, serde_json::Value> {
    let parse = |s: &str| Version::parse(s.trim()).ok().map(|v| core(&v));
    let version = core(version);
    m.flags
        .iter()
        .filter(|(_, f)| {
            let min_ok = f
                .min_version
                .as_deref()
                .is_none_or(|min| parse(min).is_some_and(|min| version >= min));
            let max_ok = f
                .max_version
                .as_deref()
                .is_none_or(|max| parse(max).is_some_and(|max| version <= max));
            let target_ok = f.target.as_deref().is_none_or(|t| t == target);
            let rollout_ok = f.rollout_percent >= 100
                || (f.rollout_percent > 0
                    && (u32::from(bucket) + f.bucket_offset) % 100 < f.rollout_percent);
            min_ok && max_ok && target_ok && rollout_ok
        })
        .map(|(name, f)| (name.clone(), f.value.clone()))
        .collect()
}

pub fn manifest_url(channel: &str) -> String {
    format!("{}/manifest/{channel}.json", links::DL_BASE)
}

pub fn signature_url(channel: &str) -> String {
    format!("{}.sig", manifest_url(channel))
}

/// The release's file in the Tauri static updater format.
pub fn tauri_url(version: &str, target: &str, arch: &str) -> String {
    format!("{}/{version}/tauri-{target}-{arch}.json", links::DL_BASE)
}

/// The installer's expected SHA-256 and size, from the signed manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    pub sha256: [u8; 32],
    pub size: u64,
}

impl Expected {
    pub fn from_platform(p: &Platform) -> Result<Self, String> {
        if !p.url.starts_with("https://") {
            return Err("The update server sent a download address that isn't https.".into());
        }
        let sha256 = parse_hex32(&p.sha256)
            .ok_or_else(|| "The release list has no valid checksum for this update.".to_string())?;
        Ok(Self {
            sha256,
            size: p.size,
        })
    }

    /// The downloaded bytes are the ones the signed manifest names.
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        if bytes.len() as u64 != self.size || hash != self.sha256 {
            return Err("The update doesn't match the signed release list.".into());
        }
        Ok(())
    }

    pub fn sha256_hex(&self) -> String {
        self.sha256.iter().map(|b| format!("{b:02x}")).collect()
    }
}

pub fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// A fixed test key: never trusted by a real build.
    pub fn test_pair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap()
    }

    pub fn test_keys() -> &'static [ManifestKey] {
        use base64::Engine;
        let public =
            base64::engine::general_purpose::STANDARD.encode(test_pair().public_key().as_ref());
        Box::leak(Box::new([ManifestKey {
            id: "test",
            public: Box::leak(public.into_boxed_str()),
        }]))
    }

    pub fn sign(body: &[u8]) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .encode(test_pair().sign(body).as_ref())
            .into_bytes()
    }

    pub fn manifest_json(generated_at: &str, extra: &str) -> String {
        format!(
            r#"{{"schema":2,"product":"unflub","channel":"stable","generated_at":"{generated_at}",
                "expires_at":"2999-01-01T00:00:00Z","key_id":"test","updates_enabled":true,"min_version":null,"blocked":[],
                "releases":[],"flags":{{}},"message":null{extra}}}"#
        )
    }

    #[test]
    fn a_good_signature_verifies() {
        let body = manifest_json("2026-11-17T12:00:00Z", "");
        let m = verify(body.as_bytes(), &sign(body.as_bytes()), test_keys()).unwrap();
        assert_eq!(m.channel, "stable");
        assert!(m.updates_enabled);
        // A raw 64-byte signature works too.
        let raw = test_pair().sign(body.as_bytes());
        assert!(verify(body.as_bytes(), raw.as_ref(), test_keys()).is_ok());
    }

    #[test]
    fn a_bad_or_missing_signature_is_rejected() {
        let body = manifest_json("2026-11-17T12:00:00Z", "");
        let sig = sign(body.as_bytes());
        // The body changed after signing.
        let tampered = body.replace("\"updates_enabled\":true", "\"updates_enabled\":false");
        assert_eq!(
            verify(tampered.as_bytes(), &sig, test_keys()),
            Err(Rejected::BadSignature)
        );
        assert_eq!(
            verify(body.as_bytes(), b"", test_keys()),
            Err(Rejected::Unsigned)
        );
        assert_eq!(
            verify(body.as_bytes(), b"  \n", test_keys()),
            Err(Rejected::Unsigned)
        );
        assert_eq!(
            verify(body.as_bytes(), b"not a signature", test_keys()),
            Err(Rejected::BadSignature)
        );
        // Signed by a key the app doesn't trust (the shipped placeholder trusts nothing).
        assert_eq!(
            verify(body.as_bytes(), &sig, crate::keys::MANIFEST_KEYS),
            Err(Rejected::BadSignature)
        );
        // A valid signature, but the manifest claims another key.
        let other = body.replace("\"key_id\":\"test\"", "\"key_id\":\"old\"");
        assert_eq!(
            verify(other.as_bytes(), &sign(other.as_bytes()), test_keys()),
            Err(Rejected::BadSignature)
        );
    }

    #[test]
    fn oversized_manifests_are_rejected() {
        let pad = " ".repeat(MAX_BYTES);
        let body = manifest_json("2026-11-17T12:00:00Z", &format!(",\"pad\":\"{pad}\""));
        assert!(body.len() > MAX_BYTES);
        assert_eq!(
            verify(body.as_bytes(), &sign(body.as_bytes()), test_keys()),
            Err(Rejected::TooLarge)
        );
    }

    #[test]
    fn signed_but_wrong_is_rejected() {
        for (body, want) in [
            (
                manifest_json("2026-11-17T12:00:00Z", "").replace("\"schema\":2", "\"schema\":1"),
                Rejected::Malformed("schema 1".into()),
            ),
            (
                manifest_json("2026-11-17T12:00:00Z", "")
                    .replace("\"product\":\"unflub\"", "\"product\":\"other\""),
                Rejected::WrongProduct,
            ),
            (
                manifest_json("yesterday", ""),
                Rejected::Malformed("generated_at".into()),
            ),
        ] {
            assert_eq!(
                verify(body.as_bytes(), &sign(body.as_bytes()), test_keys()),
                Err(want)
            );
        }
        let broken = "{ not json";
        assert!(matches!(
            verify(broken.as_bytes(), &sign(broken.as_bytes()), test_keys()),
            Err(Rejected::Malformed(_))
        ));
    }

    #[test]
    fn older_manifests_never_replace_newer_ones() {
        let at = |t: &str| {
            let body = manifest_json(t, "");
            verify(body.as_bytes(), &sign(body.as_bytes()), test_keys()).unwrap()
        };
        let noon = at("2026-11-17T12:00:00Z");
        assert_eq!(replaces(&noon, None), Ok(()));
        assert_eq!(replaces(&at("2026-11-17T12:00:01Z"), Some(&noon)), Ok(()));
        assert_eq!(replaces(&noon, Some(&noon)), Ok(()), "the same one again");
        assert_eq!(
            replaces(&at("2026-11-17T11:59:59Z"), Some(&noon)),
            Err(Rejected::Older)
        );
        // Offsets are compared as instants.
        assert_eq!(
            replaces(&at("2026-11-17T07:00:00-05:00"), Some(&noon)),
            Ok(())
        );
        assert_eq!(
            replaces(&at("2026-11-17T06:59:00-05:00"), Some(&noon)),
            Err(Rejected::Older)
        );
    }

    fn release(version: &str, rollout: u32) -> String {
        format!(
            r#"{{"version":"{version}","pub_date":"2026-11-17T12:00:00Z","notes":"n",
                "rollout_percent":{rollout},"platforms":{{"windows-x86_64":{{
                "url":"https://dl.rmgn.net/unflub/{version}/unflub_{version}_x64-setup.exe",
                "signature":"sig","sha256":"{}","size":3}}}}}}"#,
            "ab".repeat(32)
        )
    }

    fn table(
        enabled: bool,
        min: Option<&str>,
        blocked: &[&str],
        releases: &[(&str, u32)],
    ) -> Manifest {
        let json = format!(
            r#"{{"schema":2,"product":"unflub","channel":"stable","generated_at":"2026-11-17T12:00:00Z",
                "expires_at":"2999-01-01T00:00:00Z","key_id":"test","updates_enabled":{enabled},"min_version":{},"blocked":{},
                "releases":[{}]}}"#,
            min.map_or("null".into(), |m| format!("\"{m}\"")),
            serde_json::to_string(blocked).unwrap(),
            releases
                .iter()
                .map(|(v, r)| release(v, *r))
                .collect::<Vec<_>>()
                .join(",")
        );
        serde_json::from_str(&json).unwrap()
    }

    /// The server's rules, now evaluated here.
    #[test]
    fn eligibility_table() {
        let v = |s: &str| Version::parse(s).unwrap();
        let rel = [("0.3.0", 5), ("0.2.0", 100), ("0.1.0", 100)];
        let p = "windows-x86_64";
        // (enabled, min, blocked, releases, current, bucket) -> (offered, critical)
        type Case<'a> = (
            bool,
            Option<&'a str>,
            Vec<&'a str>,
            Vec<(&'a str, u32)>,
            &'a str,
            u8,
            Option<(&'a str, bool)>,
        );
        let cases: Vec<Case> = vec![
            // Kill switch.
            (false, None, vec![], rel.to_vec(), "0.1.0", 0, None),
            (
                false,
                Some("0.2.0"),
                vec!["0.1.0"],
                rel.to_vec(),
                "0.1.0",
                0,
                None,
            ),
            // Inside 0.3.0's 5%.
            (
                true,
                None,
                vec![],
                rel.to_vec(),
                "0.1.0",
                4,
                Some(("0.3.0", false)),
            ),
            // Outside it: still gets 0.2.0 at 100%.
            (
                true,
                None,
                vec![],
                rel.to_vec(),
                "0.1.0",
                5,
                Some(("0.2.0", false)),
            ),
            (
                true,
                None,
                vec![],
                rel.to_vec(),
                "0.1.0",
                99,
                Some(("0.2.0", false)),
            ),
            (true, None, vec![], rel.to_vec(), "0.2.0", 50, None),
            (true, None, vec![], rel.to_vec(), "0.3.0", 0, None),
            // 0% reaches nobody.
            (true, None, vec![], vec![("0.3.0", 0)], "0.2.0", 0, None),
            // Blocked: the newest release with rollout > 0, whatever the bucket, critical.
            (
                true,
                None,
                vec!["0.2.0"],
                rel.to_vec(),
                "0.2.0",
                99,
                Some(("0.3.0", true)),
            ),
            // Blocking the newest rolls its installs back.
            (
                true,
                None,
                vec!["0.3.0"],
                rel.to_vec(),
                "0.3.0",
                0,
                Some(("0.2.0", true)),
            ),
            // Build metadata is ignored.
            (
                true,
                None,
                vec!["0.3.0"],
                rel.to_vec(),
                "0.3.0+abc",
                0,
                Some(("0.2.0", true)),
            ),
            // The critical path skips 0% releases.
            (
                true,
                None,
                vec!["0.1.0"],
                vec![("0.3.0", 0), ("0.2.0", 100), ("0.1.0", 100)],
                "0.1.0",
                99,
                Some(("0.2.0", true)),
            ),
            // Below min_version: critical, ignores the bucket.
            (
                true,
                Some("0.2.0"),
                vec![],
                rel.to_vec(),
                "0.1.0",
                99,
                Some(("0.3.0", true)),
            ),
            (true, Some("0.2.0"), vec![], rel.to_vec(), "0.2.0", 99, None),
            // Below min, but the newest active release is this one: nothing.
            (
                true,
                Some("0.4.0"),
                vec![],
                vec![("0.3.0", 100)],
                "0.3.0",
                0,
                None,
            ),
            // A blocked release is never offered.
            (
                true,
                None,
                vec!["0.3.0"],
                rel.to_vec(),
                "0.1.0",
                0,
                Some(("0.2.0", false)),
            ),
            // Order in the file doesn't matter.
            (
                true,
                None,
                vec![],
                vec![("0.1.0", 100), ("0.3.0", 100), ("0.2.0", 100)],
                "0.1.0",
                0,
                Some(("0.3.0", false)),
            ),
        ];
        for (enabled, min, blocked, releases, current, bucket, want) in cases {
            let m = table(enabled, min, &blocked, &releases);
            let got = eligible(&m, &v(current), bucket, p)
                .map(|o| (o.release.version.clone(), o.critical));
            assert_eq!(
                got,
                want.map(|(v, c)| (v.to_string(), c)),
                "enabled {enabled}, min {min:?}, blocked {blocked:?}, {releases:?}, current {current}, bucket {bucket}"
            );
        }
        // No build for this platform: nothing.
        let m = table(true, None, &[], &rel);
        assert_eq!(eligible(&m, &v("0.1.0"), 0, "darwin-aarch64"), None);
    }

    #[test]
    fn urls_follow_the_spec() {
        assert_eq!(
            manifest_url("stable"),
            "https://dl.rmgn.net/unflub/manifest/stable.json"
        );
        assert_eq!(
            signature_url("beta"),
            "https://dl.rmgn.net/unflub/manifest/beta.json.sig"
        );
        assert_eq!(
            tauri_url("0.2.0", "windows", "x86_64"),
            "https://dl.rmgn.net/unflub/0.2.0/tauri-windows-x86_64.json"
        );
        assert_eq!(
            platform_key(),
            format!("{}-{}", crate::net::target(), std::env::consts::ARCH)
        );
    }

    #[test]
    fn expected_checks_hash_and_size() {
        use sha2::{Digest, Sha256};
        let bytes = b"installer";
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let p = Platform {
            url: "https://dl.rmgn.net/unflub/0.2.0/x.exe".into(),
            signature: "s".into(),
            sha256: hex.clone(),
            size: bytes.len() as u64,
        };
        let e = Expected::from_platform(&p).unwrap();
        assert_eq!(e.sha256_hex(), hex);
        assert!(e.check(bytes).is_ok());
        assert!(e.check(b"installe!").is_err());
        assert!(e.check(b"installer2").is_err());
        let http = Platform {
            url: "http://dl.rmgn.net/x.exe".into(),
            ..p.clone()
        };
        assert!(Expected::from_platform(&http).is_err());
        let bad_hash = Platform {
            sha256: "xyz".into(),
            ..p
        };
        assert!(Expected::from_platform(&bad_hash).is_err());
    }

    /// The API's reference cases (`rmgn-api/test/eligibility.test.ts`), one for one.
    #[test]
    fn reference_eligibility_cases() {
        let pick = |m: &Manifest, current: &str, bucket: u8| {
            eligible(
                m,
                &Version::parse(current).unwrap(),
                bucket,
                "windows-x86_64",
            )
            .map(|o| o.release.version)
        };
        let s = |v: &str| Some(v.to_string());
        let m = table(false, Some("0.2.0"), &["0.1.0"], &[("0.2.0", 100)]);
        assert_eq!(pick(&m, "0.1.0", 0), None, "kill switch");
        let m = table(true, None, &[], &[("0.2.0", 25)]);
        for (bucket, want) in [(0, s("0.2.0")), (24, s("0.2.0")), (25, None), (99, None)] {
            assert_eq!(pick(&m, "0.1.0", bucket), want, "bucket {bucket}");
        }
        assert_eq!(
            pick(&table(true, None, &[], &[("0.2.0", 0)]), "0.1.0", 0),
            None
        );
        let m = table(true, None, &[], &[("0.2.0", 100)]);
        assert_eq!(pick(&m, "0.1.0", 99), s("0.2.0"));
        let m = table(true, None, &[], &[("0.3.0", 5), ("0.2.0", 100)]);
        assert_eq!(pick(&m, "0.1.0", 3), s("0.3.0"));
        assert_eq!(pick(&m, "0.1.0", 40), s("0.2.0"));
        let m = table(
            true,
            None,
            &["0.3.0", "0.1.0"],
            &[("0.4.0", 0), ("0.3.1", 5), ("0.2.0", 100)],
        );
        assert_eq!(pick(&m, "0.1.0", 99), s("0.3.1"));
        assert_eq!(pick(&m, "0.3.0", 99), s("0.3.1"));
        let m = table(true, None, &["0.3.0"], &[("0.2.0", 100)]);
        assert_eq!(pick(&m, "0.3.0", 50), s("0.2.0"));
        assert_eq!(pick(&m, "0.3.0+win.7", 50), s("0.2.0"));
        let m = table(
            true,
            Some("0.2.0"),
            &[],
            &[("0.4.0", 0), ("0.3.0", 1), ("0.2.0", 1)],
        );
        assert_eq!(pick(&m, "0.1.5", 99), s("0.3.0"));
        assert_eq!(pick(&m, "0.2.0", 50), None);
        let m = table(true, Some("1.0.0"), &[], &[("1.0.0", 10)]);
        assert_eq!(pick(&m, "1.0.0-rc.1", 99), s("1.0.0"));
        // Below min with nothing newer started: the ordinary rollout still applies.
        let m = table(true, Some("0.9.0"), &[], &[("0.3.0", 0), ("0.2.0", 50)]);
        assert_eq!(pick(&m, "0.1.0", 10), s("0.2.0"));
        assert_eq!(
            eligible(&m, &Version::parse("0.1.0").unwrap(), 10, "windows-x86_64")
                .map(|o| o.critical),
            Some(true)
        );
    }

    fn with_flags(flags: &str) -> Manifest {
        let body = manifest_json("2026-11-17T12:00:00Z", "")
            .replace("\"flags\":{}", &format!("\"flags\":{flags}"));
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn flags_target_by_version_range_os_and_bucket() {
        let m = with_flags(
            r#"{"ranged":{"value":true,"min_version":"0.2.0","max_version":"0.3.0","target":null,"rollout_percent":100,"bucket_offset":0},
                "mac":{"value":"x","min_version":null,"max_version":null,"target":"darwin","rollout_percent":100,"bucket_offset":0}}"#,
        );
        let at = |v: &str, target: &str| resolve_flags(&m, &Version::parse(v).unwrap(), target, 0);
        assert!(!at("0.1.9", "windows").contains_key("ranged"));
        assert!(!at("0.2.0-rc.1", "windows").contains_key("ranged"));
        assert_eq!(at("0.2.0", "windows")["ranged"], true);
        assert_eq!(at("0.3.0", "windows")["ranged"], true);
        assert!(!at("0.3.1", "windows").contains_key("ranged"));
        assert_eq!(at("0.1.0", "darwin")["mac"], "x");
        assert!(!at("0.1.0", "windows").contains_key("mac"));

        let m = with_flags(
            r#"{"exp":{"value":false,"min_version":null,"max_version":null,"target":null,"rollout_percent":30,"bucket_offset":85}}"#,
        );
        let v = Version::parse("0.1.0").unwrap();
        let inside = (0..100u8)
            .filter(|&b| {
                let on = resolve_flags(&m, &v, "windows", b).contains_key("exp");
                assert_eq!(on, (u32::from(b) + 85) % 100 < 30, "bucket {b}");
                on
            })
            .count();
        assert_eq!(inside, 30);
    }

    #[test]
    fn expiry_is_required_and_checked() {
        let body = manifest_json("2026-11-17T12:00:00Z", "");
        let m = verify(body.as_bytes(), &sign(body.as_bytes()), test_keys()).unwrap();
        let now = time::OffsetDateTime::now_utc();
        assert!(!m.expired(now));
        let old = body.replace("2999-01-01T00:00:00Z", "2026-11-24T00:00:00Z");
        let m = verify(old.as_bytes(), &sign(old.as_bytes()), test_keys()).unwrap();
        assert!(m.expired(rfc3339("2026-11-24T00:00:00Z").unwrap()));
        assert!(!m.expired(rfc3339("2026-11-23T23:59:59Z").unwrap()));
        let none = body.replace("\"expires_at\":\"2999-01-01T00:00:00Z\",", "");
        assert!(matches!(
            verify(none.as_bytes(), &sign(none.as_bytes()), test_keys()),
            Err(Rejected::Malformed(_))
        ));
    }
}
