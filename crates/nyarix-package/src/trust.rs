//! Trust levels for loaded packages (see issue #63).
//!
//! **Scope note:** [`classify`] only derives what a package's Ed25519
//! signature (#61/#62) can actually tell you: whether it's signed by a
//! key this [`TrustStore`] already recognizes as
//! [`TrustLevel::Official`]/[`TrustLevel::Community`], or otherwise
//! [`TrustLevel::Unknown`]. [`TrustLevel::System`] (built into the
//! Runtime), [`TrustLevel::Local`] (loaded from a local dev directory),
//! and [`TrustLevel::Experimental`] (the user explicitly allowed it
//! anyway) aren't things a signature can express — a package's bytes
//! alone can't tell you it's "the one compiled into this Runtime
//! binary" or "the user said yes to this specific one". Whichever code
//! path knows that context (Module Loader integration, #41; a
//! not-yet-built config surface for user overrides) assigns those three
//! directly instead of calling `classify`.

use std::collections::HashMap;

use nyarix_error::PackageError;

use crate::archive::{PackageReader, SignatureStatus};
use crate::signing::{PUBLIC_KEY_MEMBER_PATH, VerifyingKey};

/// How a loaded package is trusted.
///
/// Deliberately not ordered: the issue lists six categories without
/// specifying a hierarchy between them, and asserting one (e.g. "is
/// `Local` more trusted than `Community`?") isn't this issue's call to
/// make — that's a Sandbox/capability-granting policy decision for
/// later (M7), once there's something concrete to weigh it against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrustLevel {
    /// Built into the Runtime itself.
    System,
    /// Signed with a Nyarix-controlled official key.
    Official,
    /// Signed with a key vetted by community moderators.
    Community,
    /// Unsigned, or signed by a key this [`TrustStore`] doesn't
    /// recognize.
    Unknown,
    /// Developed locally (e.g. loaded straight from a dev directory,
    /// not installed from a package source).
    Local,
    /// Otherwise untrusted, but the user explicitly allowed it anyway.
    Experimental,
}

/// A registry of public keys this Runtime recognizes, and the trust
/// level each implies (e.g. "the official Nyarix signing key" →
/// [`TrustLevel::Official`]).
///
/// Only [`TrustLevel::Official`] and [`TrustLevel::Community`] make
/// sense to register here — the other four aren't derived from a
/// public key at all (see this module's scope note).
#[derive(Debug, Default)]
pub struct TrustStore {
    known_keys: HashMap<[u8; 32], TrustLevel>,
}

impl TrustStore {
    /// An empty store: no key is recognized yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `key` as implying `level` (expected to be
    /// [`TrustLevel::Official`] or [`TrustLevel::Community`] — nothing
    /// stops registering another variant, but [`classify`] never
    /// produces [`TrustLevel::System`]/[`TrustLevel::Local`]/
    /// [`TrustLevel::Experimental`] from a lookup, so registering those
    /// here would just never be observed through `classify`).
    pub fn trust(&mut self, key: VerifyingKey, level: TrustLevel) {
        self.known_keys.insert(key.to_bytes(), level);
    }

    /// The trust level registered for `key`, if any.
    #[must_use]
    pub fn level_for(&self, key: &VerifyingKey) -> Option<TrustLevel> {
        self.known_keys.get(&key.to_bytes()).copied()
    }
}

/// Classify `reader` using only what its embedded signature can prove:
/// a valid signature by a key in `trust_store` gets that key's
/// registered level; anything else — unsigned, invalid, or signed by an
/// unrecognized key — is [`TrustLevel::Unknown`].
///
/// # Errors
/// Returns [`PackageError`] if re-reading the archive to check its
/// signature fails (see [`PackageReader::signature_status`]).
pub fn classify(
    reader: &PackageReader,
    trust_store: &TrustStore,
) -> Result<TrustLevel, PackageError> {
    if reader.signature_status()? != SignatureStatus::Verified {
        return Ok(TrustLevel::Unknown);
    }

    let Some(public_key_bytes) = reader.read_entry(PUBLIC_KEY_MEMBER_PATH)? else {
        return Ok(TrustLevel::Unknown);
    };
    let Ok(public_key_bytes): Result<[u8; 32], _> = public_key_bytes.try_into() else {
        return Ok(TrustLevel::Unknown);
    };
    let Ok(public_key) = VerifyingKey::from_bytes(&public_key_bytes) else {
        return Ok(TrustLevel::Unknown);
    };

    Ok(trust_store
        .level_for(&public_key)
        .unwrap_or(TrustLevel::Unknown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::PackageBuilder;
    use crate::signing;

    const MANIFEST: &str = r#"
[package]
name = "nyarix-transport-udp"
version = "0.1.0"
module_type = "transport"
api_version = "1.0"
author = "Nyarix"
description = "UDP transport"
"#;

    fn build_signed(signing_key: &signing::SigningKey) -> Vec<u8> {
        PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .sign(signing_key)
            .build()
            .unwrap()
    }

    #[test]
    fn an_unsigned_package_is_unknown() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .build()
            .unwrap();
        let reader = PackageReader::open(&data).unwrap();

        let level = classify(&reader, &TrustStore::new()).unwrap();

        assert_eq!(level, TrustLevel::Unknown);
    }

    #[test]
    fn a_signed_package_from_an_unrecognized_key_is_unknown() {
        let key = signing::generate_keypair();
        let data = build_signed(&key);
        let reader = PackageReader::open(&data).unwrap();

        let level = classify(&reader, &TrustStore::new()).unwrap();

        assert_eq!(level, TrustLevel::Unknown);
    }

    #[test]
    fn a_signed_package_from_an_official_key_is_official() {
        let key = signing::generate_keypair();
        let data = build_signed(&key);
        let reader = PackageReader::open(&data).unwrap();

        let mut store = TrustStore::new();
        store.trust(key.verifying_key(), TrustLevel::Official);

        let level = classify(&reader, &store).unwrap();

        assert_eq!(level, TrustLevel::Official);
    }

    #[test]
    fn a_signed_package_from_a_community_key_is_community() {
        let key = signing::generate_keypair();
        let data = build_signed(&key);
        let reader = PackageReader::open(&data).unwrap();

        let mut store = TrustStore::new();
        store.trust(key.verifying_key(), TrustLevel::Community);

        let level = classify(&reader, &store).unwrap();

        assert_eq!(level, TrustLevel::Community);
    }

    #[test]
    fn a_package_signed_by_a_different_key_than_registered_is_unknown() {
        let signer = signing::generate_keypair();
        let registered = signing::generate_keypair();
        let data = build_signed(&signer);
        let reader = PackageReader::open(&data).unwrap();

        let mut store = TrustStore::new();
        store.trust(registered.verifying_key(), TrustLevel::Official);

        let level = classify(&reader, &store).unwrap();

        assert_eq!(level, TrustLevel::Unknown);
    }
}
