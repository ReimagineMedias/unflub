//! Public keys that sign the update manifest (`manifest.rs`).
//!
//! The manifest at `dl.rmgn.net/unflub/manifest/{channel}.json` carries the release list, the
//! rollout, the kill switch and the remote flags, so it is only believed with a valid Ed25519
//! signature (`{channel}.json.sig`, over the exact bytes) from one of these keys. The manifest's
//! `key_id` must name the key that verified it, so a key can be replaced by shipping a version
//! that trusts both, then signing with the new one.
//!
//! Each key is the raw 32-byte Ed25519 public key in standard base64 (44 characters) and its id
//! is the first 16 hex characters of the key's SHA-256, exactly what
//! `D:\Code\rmgn-api\scripts\keys.mjs` prints (`MANIFEST_KEYS = { "<id>": "<base64>" }`). The
//! private key lives only in the API's `MANIFEST_SIGNING_KEY` secret.
//!
//! **The entry below is a placeholder** (all zeros, which verifies nothing): until the real key
//! replaces it, every manifest is rejected, so no update, flag or message is ever accepted. The
//! release workflow refuses to build while it is here.

pub struct ManifestKey {
    pub id: &'static str,
    /// Raw Ed25519 public key, standard base64.
    pub public: &'static str,
}

pub const MANIFEST_KEYS: &[ManifestKey] = &[ManifestKey {
    id: "placeholder",
    public: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
}];

/// The decoded key, or `None` when it isn't 32 bytes of base64.
pub fn decode(public: &str) -> Option<[u8; 32]> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(public.trim())
        .ok()?
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_decode_and_ids_are_unique() {
        for key in MANIFEST_KEYS {
            assert!(decode(key.public).is_some(), "{} is not 32 bytes", key.id);
            assert!(!key.id.is_empty());
        }
        let ids: std::collections::BTreeSet<_> = MANIFEST_KEYS.iter().map(|k| k.id).collect();
        assert_eq!(ids.len(), MANIFEST_KEYS.len());
    }

    /// Real keys carry the id `keys.mjs` gives them: the first 8 bytes of SHA-256(key), in hex.
    #[test]
    fn key_ids_match_their_keys() {
        use sha2::{Digest, Sha256};
        for key in MANIFEST_KEYS.iter().filter(|k| k.id != "placeholder") {
            let digest = Sha256::digest(decode(key.public).unwrap());
            let id: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(key.id, id);
        }
    }

    #[test]
    fn decode_rejects_wrong_lengths() {
        assert_eq!(decode("AAAA"), None);
        assert_eq!(decode("not base64!"), None);
    }
}
