//! Package pack/unpack: `tar` + `zstd` (see issue #60).

use std::io::{Cursor, Read};

use nyarix_error::PackageError;

use crate::manifest::PackageManifest;
use crate::signing::{self, PUBLIC_KEY_MEMBER_PATH, SIGNATURE_MEMBER_PATH, SigningKey};
use crate::{PackageMember, validate_layout};

fn io_error(source: std::io::Error) -> PackageError {
    PackageError::Io {
        path: "<in-memory .nyp archive>".to_string(),
        source,
    }
}

/// Builds a `.nyp` archive in memory: `tar`, then `zstd`-compressed.
#[derive(Debug, Clone, Default)]
pub struct PackageBuilder {
    entries: Vec<(String, Vec<u8>)>,
}

impl PackageBuilder {
    /// Start building an empty archive.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a file at `path` (e.g. `"manifest.toml"`, `"payload/module.wasm"`).
    ///
    /// Files are written to the archive in the order they're added.
    #[must_use]
    pub fn add_file(mut self, path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        self.entries.push((path.into(), contents.into()));
        self
    }

    /// Sign every file added so far with `signing_key` (#61), and add
    /// the resulting signature and its public key as two new members
    /// ([`SIGNATURE_MEMBER_PATH`], [`PUBLIC_KEY_MEMBER_PATH`]).
    ///
    /// Call this after adding every other file and before [`Self::build`]
    /// — anything added after signing isn't covered by the signature.
    #[must_use]
    pub fn sign(self, signing_key: &SigningKey) -> Self {
        let signature = signing::sign(&self.entries, signing_key);
        self.add_file(SIGNATURE_MEMBER_PATH, signature.to_bytes().to_vec())
            .add_file(
                PUBLIC_KEY_MEMBER_PATH,
                signing_key.verifying_key().to_bytes().to_vec(),
            )
    }

    /// Build the archive: `tar` all added files, then `zstd`-compress the
    /// result.
    ///
    /// # Errors
    /// Returns [`PackageError::Io`] if writing the `tar` stream or
    /// `zstd`-compressing it fails (both operate on in-memory buffers, so
    /// this should only happen on allocation failure or a malformed
    /// path).
    pub fn build(&self) -> Result<Vec<u8>, PackageError> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, contents) in &self.entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, contents.as_slice())
                    .map_err(io_error)?;
            }
            builder.finish().map_err(io_error)?;
        }
        zstd::stream::encode_all(Cursor::new(tar_bytes), 0).map_err(io_error)
    }
}

/// Reads a `.nyp` archive: `zstd`-decompresses it, validates its layout
/// (#58), and parses `manifest.toml` (#59) up front.
///
/// Other members (`payload/`, `assets/`, `signatures/`) are **not**
/// eagerly read into memory — only their presence is checked during
/// [`Self::open`]. Fetch one with [`Self::read_entry`] when actually
/// needed; this is the "read manifest without full unpack" the issue
/// asks for.
#[derive(Debug)]
pub struct PackageReader {
    /// The decompressed `tar` stream, kept so [`Self::read_entry`] can
    /// re-scan it on demand instead of holding every entry's bytes.
    tar_bytes: Vec<u8>,
    manifest: PackageManifest,
}

impl PackageReader {
    /// Open a `.nyp` archive from its raw (compressed) bytes.
    ///
    /// # Errors
    /// Returns [`PackageError::Io`] if `data` isn't valid `zstd`/`tar`,
    /// [`PackageError::MissingMember`] if `manifest.toml` (the only
    /// required member, #58) isn't present, or
    /// [`PackageError::InvalidManifest`] if it's present but doesn't
    /// parse (#59).
    pub fn open(data: &[u8]) -> Result<Self, PackageError> {
        let tar_bytes = zstd::stream::decode_all(Cursor::new(data)).map_err(io_error)?;

        let mut archive = tar::Archive::new(Cursor::new(tar_bytes.as_slice()));
        let mut entry_paths = Vec::new();
        let mut manifest_raw = None;
        for entry in archive.entries().map_err(io_error)? {
            let mut entry = entry.map_err(io_error)?;
            let path = entry
                .path()
                .map_err(io_error)?
                .to_string_lossy()
                .into_owned();
            if path == PackageMember::Manifest.path() {
                let mut buf = String::new();
                entry.read_to_string(&mut buf).map_err(io_error)?;
                manifest_raw = Some(buf);
            }
            entry_paths.push(path);
        }

        validate_layout(&entry_paths)?;
        // `validate_layout` already guarantees `manifest.toml` was found
        // among `entry_paths`, so `manifest_raw` is always `Some` here.
        let manifest = PackageManifest::from_toml(
            &manifest_raw.expect("validate_layout guarantees manifest.toml is present"),
        )?;

        Ok(Self {
            tar_bytes,
            manifest,
        })
    }

    /// The package's parsed manifest.
    #[must_use]
    pub const fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    /// Read a single member's raw bytes by path, without materializing
    /// any other member.
    ///
    /// Returns `Ok(None)` if no entry with that exact path exists.
    ///
    /// # Errors
    /// Returns [`PackageError::Io`] if the archive can't be re-scanned
    /// (it was already validated once in [`Self::open`], so this should
    /// only happen on allocation failure).
    pub fn read_entry(&self, path: &str) -> Result<Option<Vec<u8>>, PackageError> {
        let mut archive = tar::Archive::new(Cursor::new(self.tar_bytes.as_slice()));
        for entry in archive.entries().map_err(io_error)? {
            let mut entry = entry.map_err(io_error)?;
            let entry_path = entry
                .path()
                .map_err(io_error)?
                .to_string_lossy()
                .into_owned();
            if entry_path == path {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).map_err(io_error)?;
                return Ok(Some(buf));
            }
        }
        Ok(None)
    }

    /// Read every member's path and raw bytes.
    ///
    /// Unlike [`Self::read_entry`], this does materialize the whole
    /// archive in memory — it's what [`Self::signature_status`] (#62)
    /// needs, since a signature covers every other member.
    ///
    /// # Errors
    /// Returns [`PackageError::Io`] if the archive can't be re-scanned.
    pub fn entries(&self) -> Result<Vec<(String, Vec<u8>)>, PackageError> {
        let mut archive = tar::Archive::new(Cursor::new(self.tar_bytes.as_slice()));
        let mut entries = Vec::new();
        for entry in archive.entries().map_err(io_error)? {
            let mut entry = entry.map_err(io_error)?;
            let path = entry
                .path()
                .map_err(io_error)?
                .to_string_lossy()
                .into_owned();
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(io_error)?;
            entries.push((path, buf));
        }
        Ok(entries)
    }

    /// Check this package's embedded Ed25519 signature (#61), if any.
    ///
    /// This only checks that the signature — if present — verifies
    /// against the public key bundled alongside it; it does **not**
    /// decide whether that key should be trusted (Trust levels, #63).
    ///
    /// # Errors
    /// Returns [`PackageError::Io`] if the archive can't be re-scanned.
    pub fn signature_status(&self) -> Result<SignatureStatus, PackageError> {
        let entries = self.entries()?;
        let signature_bytes = entries
            .iter()
            .find(|(path, _)| path == signing::SIGNATURE_MEMBER_PATH)
            .map(|(_, contents)| contents);
        let public_key_bytes = entries
            .iter()
            .find(|(path, _)| path == signing::PUBLIC_KEY_MEMBER_PATH)
            .map(|(_, contents)| contents);

        let (Some(signature_bytes), Some(public_key_bytes)) = (signature_bytes, public_key_bytes)
        else {
            return Ok(SignatureStatus::Unsigned);
        };

        let Ok(signature_bytes): Result<[u8; 64], _> = signature_bytes.as_slice().try_into()
        else {
            return Ok(SignatureStatus::Invalid);
        };
        let Ok(public_key_bytes): Result<[u8; 32], _> = public_key_bytes.as_slice().try_into()
        else {
            return Ok(SignatureStatus::Invalid);
        };
        let signature = signing::Signature::from_bytes(&signature_bytes);
        let Ok(public_key) = signing::VerifyingKey::from_bytes(&public_key_bytes) else {
            return Ok(SignatureStatus::Invalid);
        };

        Ok(
            match signing::verify(&entries, &signature, &public_key) {
                Ok(()) => SignatureStatus::Verified,
                Err(signing::SignatureVerificationFailed) => SignatureStatus::Invalid,
            },
        )
    }

    /// Require this package to carry a signature that verifies —
    /// "production: enforce" strict mode (#62). "dev: skip" is simply
    /// not calling this and relying on [`Self::open`] alone; there's no
    /// separate mode flag here because the decision belongs to whatever
    /// configuration the Runtime reads, not this crate.
    ///
    /// # Errors
    /// Returns [`PackageError::SignatureFailure`] if the package is
    /// unsigned or its signature doesn't verify.
    pub fn require_valid_signature(&self) -> Result<(), PackageError> {
        match self.signature_status()? {
            SignatureStatus::Verified => Ok(()),
            SignatureStatus::Unsigned | SignatureStatus::Invalid => {
                Err(PackageError::SignatureFailure {
                    package: self.manifest.package.name.clone(),
                })
            }
        }
    }
}

/// The outcome of checking a package's embedded signature (see
/// [`PackageReader::signature_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    /// No `signatures/ed25519.sig`/`signatures/ed25519.pub` pair present.
    Unsigned,
    /// Present, and the signature verifies against the bundled public key.
    Verified,
    /// Present, but malformed or doesn't verify (tampered, wrong key).
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[package]
name = "nyarix-transport-udp"
version = "0.1.0"
module_type = "transport"
api_version = "1.0"
author = "Nyarix"
description = "UDP transport"
"#;

    #[test]
    fn builds_and_reopens_a_minimal_package() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();
        assert_eq!(reader.manifest().package.name, "nyarix-transport-udp");
    }

    #[test]
    fn reads_a_non_manifest_entry_on_demand() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"fake wasm bytes".as_slice())
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();
        let payload = reader.read_entry("payload/module.wasm").unwrap().unwrap();
        assert_eq!(payload, b"fake wasm bytes");
    }

    #[test]
    fn read_entry_returns_none_for_a_missing_path() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();
        assert!(reader.read_entry("assets/missing.png").unwrap().is_none());
    }

    #[test]
    fn open_rejects_an_archive_without_a_manifest() {
        let data = PackageBuilder::new()
            .add_file("payload/module.wasm", b"fake wasm bytes".as_slice())
            .build()
            .unwrap();

        let err = PackageReader::open(&data).unwrap_err();
        assert!(matches!(
            err,
            PackageError::MissingMember { path } if path == "manifest.toml"
        ));
    }

    #[test]
    fn open_rejects_not_actually_zstd_compressed_data() {
        let err = PackageReader::open(b"not a zstd stream").unwrap_err();
        assert!(matches!(err, PackageError::Io { .. }));
    }

    #[test]
    fn open_rejects_a_malformed_manifest_inside_a_valid_archive() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", b"not valid toml [[[".as_slice())
            .build()
            .unwrap();

        let err = PackageReader::open(&data).unwrap_err();
        assert!(matches!(err, PackageError::InvalidManifest { .. }));
    }

    #[test]
    fn round_trips_multiple_files_in_insertion_order() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("assets/logo.png", b"pretend-png".as_slice())
            .add_file("signatures/manifest.sig", b"pretend-signature".as_slice())
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();
        assert_eq!(
            reader.read_entry("assets/logo.png").unwrap().unwrap(),
            b"pretend-png"
        );
        assert_eq!(
            reader
                .read_entry("signatures/manifest.sig")
                .unwrap()
                .unwrap(),
            b"pretend-signature"
        );
    }

    #[test]
    fn a_signed_package_embeds_a_verifiable_signature_and_public_key() {
        let signing_key = signing::generate_keypair();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"fake wasm".as_slice())
            .sign(&signing_key)
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();
        let signature_bytes = reader.read_entry(SIGNATURE_MEMBER_PATH).unwrap().unwrap();
        let public_key_bytes = reader.read_entry(PUBLIC_KEY_MEMBER_PATH).unwrap().unwrap();

        let signature = signing::Signature::from_slice(&signature_bytes).unwrap();
        let public_key =
            signing::VerifyingKey::from_bytes(&public_key_bytes.try_into().unwrap()).unwrap();

        let manifest_bytes = reader.read_entry("manifest.toml").unwrap().unwrap();
        let payload_bytes = reader.read_entry("payload/module.wasm").unwrap().unwrap();
        let entries = vec![
            ("manifest.toml".to_string(), manifest_bytes),
            ("payload/module.wasm".to_string(), payload_bytes),
        ];

        assert!(signing::verify(&entries, &signature, &public_key).is_ok());
    }

    #[test]
    fn an_unsigned_package_reports_unsigned() {
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();

        assert_eq!(reader.signature_status().unwrap(), SignatureStatus::Unsigned);
        assert!(reader.require_valid_signature().is_err());
    }

    #[test]
    fn a_validly_signed_package_reports_verified() {
        let signing_key = signing::generate_keypair();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"fake wasm".as_slice())
            .sign(&signing_key)
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();

        assert_eq!(reader.signature_status().unwrap(), SignatureStatus::Verified);
        assert!(reader.require_valid_signature().is_ok());
    }

    #[test]
    fn a_package_signed_by_a_different_key_reports_invalid() {
        let signing_key = signing::generate_keypair();
        let other_key = signing::generate_keypair();

        let signature = signing::sign(
            &[
                ("manifest.toml".to_string(), MANIFEST.as_bytes().to_vec()),
                (
                    "payload/module.wasm".to_string(),
                    b"fake wasm".to_vec(),
                ),
            ],
            &signing_key,
        );
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"fake wasm".as_slice())
            .add_file(SIGNATURE_MEMBER_PATH, signature.to_bytes().to_vec())
            .add_file(
                PUBLIC_KEY_MEMBER_PATH,
                other_key.verifying_key().to_bytes().to_vec(),
            )
            .build()
            .unwrap();

        let reader = PackageReader::open(&data).unwrap();

        assert_eq!(reader.signature_status().unwrap(), SignatureStatus::Invalid);
        assert!(reader.require_valid_signature().is_err());
    }

    #[test]
    fn a_tampered_package_reports_invalid() {
        let signing_key = signing::generate_keypair();
        let data = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"original wasm".as_slice())
            .sign(&signing_key)
            .build()
            .unwrap();

        // Simulate tampering after signing: reopen, then rebuild with
        // altered payload but the same (now-stale) signature/public key.
        let reader = PackageReader::open(&data).unwrap();
        let signature_bytes = reader.read_entry(SIGNATURE_MEMBER_PATH).unwrap().unwrap();
        let public_key_bytes = reader.read_entry(PUBLIC_KEY_MEMBER_PATH).unwrap().unwrap();

        let tampered = PackageBuilder::new()
            .add_file("manifest.toml", MANIFEST.as_bytes())
            .add_file("payload/module.wasm", b"tampered wasm".as_slice())
            .add_file(SIGNATURE_MEMBER_PATH, signature_bytes)
            .add_file(PUBLIC_KEY_MEMBER_PATH, public_key_bytes)
            .build()
            .unwrap();

        let reader = PackageReader::open(&tampered).unwrap();
        assert_eq!(reader.signature_status().unwrap(), SignatureStatus::Invalid);
    }
}
