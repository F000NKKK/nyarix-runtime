//! Ed25519 signing for `.nyp` packages (see issue #61).
//!
//! **Scope note:** this is the signing *primitive* — key generation,
//! computing a deterministic signable digest over a package's entries,
//! signing it, and a matching `verify` to test the round trip. It does
//! not implement:
//! - a `nyarix sign` CLI binary (the issue's last bullet) — this
//!   workspace has no CLI/argument-parsing dependency (`clap` or
//!   similar) and no existing CLI binary to model one on; adding both
//!   for a single subcommand is its own scope, tracked in #108;
//! - checking a signature automatically while *loading* a package, or
//!   trust levels for whose public key to accept (#62/#63) — this
//!   module only proves a signature is valid for a given public key,
//!   it doesn't decide whether that key should be trusted.

pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::{Signer, Verifier};

/// The path a package's Ed25519 signature is stored at within the
/// archive (#61).
pub const SIGNATURE_MEMBER_PATH: &str = "signatures/ed25519.sig";
/// The path a package's Ed25519 public key is stored at within the
/// archive, alongside the signature it verifies.
///
/// Bundling the public key with its own signature only proves the
/// package wasn't corrupted/tampered with in transit — it does **not**
/// establish trust on its own (anyone can generate a keypair and sign
/// anything). Deciding which public keys are actually trusted is
/// Trust levels' job (#63), which is expected to keep its own list of
/// known keys rather than blindly accept whatever key a package ships.
pub const PUBLIC_KEY_MEMBER_PATH: &str = "signatures/ed25519.pub";

/// Generate a new Ed25519 keypair using the OS's secure random source.
#[must_use]
pub fn generate_keypair() -> SigningKey {
    SigningKey::generate(&mut rand::rng())
}

/// Compute the deterministic bytes a package's signature covers.
///
/// `entries` should be every top-level member's `(path, contents)`
/// *except* any existing `signatures/` entries — a signature can't
/// cover itself. Entries are sorted by path before hashing so the
/// result doesn't depend on the order they were added to a
/// [`crate::PackageBuilder`].
#[must_use]
pub fn signable_bytes<S: AsRef<str>>(entries: &[(S, Vec<u8>)]) -> Vec<u8> {
    let mut sorted: Vec<(&str, &[u8])> = entries
        .iter()
        .filter(|(path, _)| !path.as_ref().starts_with("signatures/"))
        .map(|(path, contents)| (path.as_ref(), contents.as_slice()))
        .collect();
    sorted.sort_unstable_by_key(|(path, _)| *path);

    let mut buf = Vec::new();
    for (path, contents) in sorted {
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        buf.extend_from_slice(contents);
    }
    buf
}

/// Sign `entries` (see [`signable_bytes`]) with `signing_key`.
#[must_use]
pub fn sign<S: AsRef<str>>(entries: &[(S, Vec<u8>)], signing_key: &SigningKey) -> Signature {
    signing_key.sign(&signable_bytes(entries))
}

/// A signature did not verify against the given public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("signature does not verify against the given public key")]
pub struct SignatureVerificationFailed;

/// Verify that `signature` is a valid Ed25519 signature over `entries`
/// (see [`signable_bytes`]) by `verifying_key`.
///
/// # Errors
/// Returns [`SignatureVerificationFailed`] if it isn't.
pub fn verify<S: AsRef<str>>(
    entries: &[(S, Vec<u8>)],
    signature: &Signature,
    verifying_key: &VerifyingKey,
) -> Result<(), SignatureVerificationFailed> {
    verifying_key
        .verify(&signable_bytes(entries), signature)
        .map_err(|_| SignatureVerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<(String, Vec<u8>)> {
        vec![
            ("manifest.toml".to_string(), b"package data".to_vec()),
            ("payload/module.wasm".to_string(), b"fake wasm".to_vec()),
        ]
    }

    #[test]
    fn a_valid_signature_verifies() {
        let key = generate_keypair();
        let entries = entries();

        let signature = sign(&entries, &key);

        assert!(verify(&entries, &signature, &key.verifying_key()).is_ok());
    }

    #[test]
    fn a_signature_from_a_different_key_does_not_verify() {
        let key = generate_keypair();
        let other_key = generate_keypair();
        let entries = entries();

        let signature = sign(&entries, &key);

        assert!(verify(&entries, &signature, &other_key.verifying_key()).is_err());
    }

    #[test]
    fn tampering_with_an_entry_invalidates_the_signature() {
        let key = generate_keypair();
        let entries = entries();
        let signature = sign(&entries, &key);

        let mut tampered = entries;
        tampered[0].1 = b"tampered data".to_vec();

        assert!(verify(&tampered, &signature, &key.verifying_key()).is_err());
    }

    #[test]
    fn signable_bytes_is_independent_of_entry_order() {
        let mut a = entries();
        let mut b = entries();
        b.reverse();
        assert_ne!(a, b, "test setup should actually reorder entries");

        assert_eq!(signable_bytes(&a), signable_bytes(&b));
        a.clear();
        b.clear();
    }

    #[test]
    fn signable_bytes_excludes_existing_signature_entries() {
        let base = entries();
        let mut with_signature = base.clone();
        with_signature.push((SIGNATURE_MEMBER_PATH.to_string(), b"some-signature".to_vec()));

        assert_eq!(signable_bytes(&base), signable_bytes(&with_signature));
    }

    #[test]
    fn keypair_generation_produces_a_usable_verifying_key() {
        let key = generate_keypair();
        let message = b"hello";
        let signature = key.sign(message);
        assert!(key.verifying_key().verify(message, &signature).is_ok());
    }
}
