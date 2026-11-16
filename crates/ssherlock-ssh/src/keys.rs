//! SSH keys for the keychain: generating, importing, exporting and fingerprints (KEY-04…06,
//! KEY-13).
//!
//! **Service** layer of the keychain feature. Everything here works on key text in memory and
//! hands back [`KeyMaterial`] for the store to seal; nothing touches the vault or the disk.
//! Generating RSA keys and decrypting passphrase-protected ones take a noticeable time, so the
//! functions are `_blocking`: run them on a background thread.

use std::fmt;

use md5::Digest as _;
use rand::SeedableRng as _;
use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
use russh::keys::ssh_key::{self, LineEnding};
use russh::keys::{Algorithm, EcdsaCurve as SshCurve, HashAlg, PrivateKey, PublicKey};
use ssherlock_core::keychain::{EcdsaCurve, KeyKind};
use ssherlock_core::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::SshError;
use crate::known_hosts::fingerprint;

/// A key ready to save: its public half and fingerprint in the clear, its private half as
/// OpenSSH key text that the store seals.
pub struct KeyMaterial {
    /// The algorithm and size.
    pub kind: KeyKind,
    /// One OpenSSH line, `ssh-ed25519 AAAA… comment`.
    pub public_key: String,
    /// `SHA256:…`.
    pub fingerprint: String,
    /// The private key in OpenSSH format, encrypted with its passphrase when
    /// [`is_encrypted`](Self::is_encrypted).
    pub private_key: SecretString,
    /// Whether the private key needs a passphrase.
    pub is_encrypted: bool,
}

impl fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyMaterial")
            .field("kind", &self.kind)
            .field("public_key", &self.public_key)
            .field("fingerprint", &self.fingerprint)
            .field("private_key", &"[redacted]")
            .field("is_encrypted", &self.is_encrypted)
            .finish()
    }
}

/// Makes a new key of `kind` with `comment`, encrypted with `passphrase` if one is given
/// (KEY-04).
///
/// # Errors
///
/// [`SshError::KeyUnsupported`] for an RSA size outside 2048–4096, [`SshError::Random`] if the
/// OS can't supply randomness, or [`SshError::KeyGenerate`] if generation fails.
pub fn generate_blocking(
    kind: KeyKind,
    comment: &str,
    passphrase: Option<&SecretString>,
) -> Result<KeyMaterial, SshError> {
    let failed = |source| SshError::KeyGenerate {
        kind: kind.display_name(),
        source,
    };
    // A CSPRNG seeded fresh from the OS for this one key, and dropped with it. ssh-key wants an
    // infallible generator; seeding is the one step that can fail, and it reports instead of
    // panicking.
    let mut rng = rand::rngs::StdRng::try_from_rng(&mut rand::rngs::SysRng)?;
    let key = match kind {
        KeyKind::Rsa(bits) => {
            if !(KeyKind::RSA_MIN_BITS..=KeyKind::RSA_MAX_BITS).contains(&bits) {
                return Err(SshError::KeyUnsupported {
                    algorithm: kind.display_name(),
                });
            }
            let bits = usize::try_from(bits).map_err(|_| SshError::KeyUnsupported {
                algorithm: kind.display_name(),
            })?;
            let pair = RsaKeypair::random(&mut rng, bits).map_err(failed)?;
            PrivateKey::new(KeypairData::from(pair), comment).map_err(failed)?
        }
        KeyKind::Ed25519 | KeyKind::Ecdsa(_) => {
            let mut key = PrivateKey::random(&mut rng, algorithm(kind)).map_err(failed)?;
            key.set_comment(comment);
            key
        }
    };
    material(&key, passphrase)
}

/// Reads a private key pasted or loaded from a file (KEY-05): OpenSSH (encrypted or not),
/// PKCS#8 (encrypted or not), PKCS#1 RSA PEM (encrypted or not), SEC1 EC PEM (unencrypted;
/// russh can't decrypt legacy encrypted EC PEM), or a `PuTTY` `.ppk`.
///
/// An encrypted OpenSSH key is kept as it is, still encrypted, and needs no passphrase to
/// import (its public half is in the clear); a passphrase given is checked. Other formats are
/// converted to OpenSSH, and re-encrypted with the passphrase they were decrypted with.
///
/// # Errors
///
/// [`SshError::KeyNeedsPassphrase`] if the key is encrypted in a format that needs the
/// passphrase to read, [`SshError::KeyWrongPassphrase`] if the one given is wrong,
/// [`SshError::KeyUnreadable`] if the text isn't a private key, or
/// [`SshError::KeyUnsupported`] for an algorithm the keychain doesn't hold.
pub fn import_blocking(
    text: &SecretString,
    passphrase: Option<&SecretString>,
) -> Result<KeyMaterial, SshError> {
    let text = text.expose_secret().trim();
    if let Ok(key) = PrivateKey::from_openssh(text) {
        if key.is_encrypted()
            && let Some(passphrase) = passphrase
        {
            key.decrypt(passphrase.expose_secret())
                .map_err(|_| SshError::KeyWrongPassphrase)?;
        }
        let kind = kind_of(key.public_key())?;
        return Ok(KeyMaterial {
            kind,
            public_key: public_line(key.public_key())?,
            fingerprint: fingerprint(key.public_key()),
            private_key: SecretString::from(format!("{text}\n")),
            is_encrypted: key.is_encrypted(),
        });
    }
    let key = russh::keys::decode_secret_key(text, passphrase.map(ExposeSecret::expose_secret))
        .map_err(|error| match error {
            russh::keys::Error::KeyIsEncrypted => SshError::KeyNeedsPassphrase,
            _ if passphrase.is_some() && text.contains("ENCRYPTED") => SshError::KeyWrongPassphrase,
            other => SshError::KeyUnreadable {
                reason: other.to_string(),
            },
        })?;
    material(&key, passphrase)
}

/// The formats a private key can be exported in (KEY-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// OpenSSH's own format, as stored, passphrase and all.
    OpenSsh,
    /// PKCS#8 PEM (`BEGIN PRIVATE KEY`, or `BEGIN ENCRYPTED PRIVATE KEY` with a passphrase),
    /// for tools that don't read OpenSSH keys.
    Pkcs8,
}

/// The private key text to save for `format`. `private_key` is the stored OpenSSH text and
/// `passphrase` decrypts it if it's encrypted; a PKCS#8 export is encrypted with the same
/// passphrase.
///
/// # Errors
///
/// [`SshError::KeyNeedsPassphrase`] or [`SshError::KeyWrongPassphrase`] for an encrypted key
/// exported as PKCS#8, or [`SshError::KeyExport`] if encoding fails.
pub fn export_blocking(
    private_key: &SecretString,
    passphrase: Option<&SecretString>,
    format: ExportFormat,
) -> Result<SecretString, SshError> {
    if format == ExportFormat::OpenSsh {
        return Ok(private_key.clone());
    }
    let key = decrypted(private_key, passphrase)?;
    let mut pem = Zeroizing::new(Vec::new());
    let written = match passphrase.filter(|_| key.1) {
        Some(passphrase) => russh::keys::encode_pkcs8_pem_encrypted(
            &key.0,
            passphrase.expose_secret().as_bytes(),
            PKCS8_KDF_ROUNDS,
            &mut *pem,
        ),
        None => russh::keys::encode_pkcs8_pem(&key.0, &mut *pem),
    };
    written.map_err(|e| SshError::KeyExport {
        reason: e.to_string(),
    })?;
    let text = std::str::from_utf8(&pem).map_err(|e| SshError::KeyExport {
        reason: e.to_string(),
    })?;
    Ok(SecretString::from(text))
}

/// PBKDF2 rounds for an encrypted PKCS#8 export; OpenSSL's default is far lower, and these
/// files sit on disk.
const PKCS8_KDF_ROUNDS: u32 = 100_000;

/// Opens the stored OpenSSH text for signing: decrypts it with `passphrase` when it's
/// encrypted. Returns the key and whether it was encrypted.
fn decrypted(
    private_key: &SecretString,
    passphrase: Option<&SecretString>,
) -> Result<(PrivateKey, bool), SshError> {
    let key = PrivateKey::from_openssh(private_key.expose_secret()).map_err(|e| {
        SshError::KeyUnreadable {
            reason: e.to_string(),
        }
    })?;
    if !key.is_encrypted() {
        return Ok((key, false));
    }
    let passphrase = passphrase.ok_or(SshError::KeyNeedsPassphrase)?;
    let key = key
        .decrypt(passphrase.expose_secret())
        .map_err(|_| SshError::KeyWrongPassphrase)?;
    Ok((key, true))
}

/// Opens a stored key for logging in: parses the stored OpenSSH text and decrypts it with
/// `passphrase` when it's encrypted.
///
/// # Errors
///
/// [`SshError::KeyNeedsPassphrase`] or [`SshError::KeyWrongPassphrase`] for an encrypted key,
/// [`SshError::KeyUnreadable`] if the text isn't an OpenSSH private key.
pub fn open_blocking(
    private_key: &SecretString,
    passphrase: Option<&SecretString>,
) -> Result<PrivateKey, SshError> {
    decrypted(private_key, passphrase).map(|(key, _)| key)
}

/// Whether a stored key is encrypted, without decrypting it.
///
/// # Errors
///
/// [`SshError::KeyUnreadable`] if the text isn't an OpenSSH private key.
pub fn is_encrypted(private_key: &SecretString) -> Result<bool, SshError> {
    PrivateKey::from_openssh(private_key.expose_secret())
        .map(|key| key.is_encrypted())
        .map_err(|e| SshError::KeyUnreadable {
            reason: e.to_string(),
        })
}

/// Fingerprints and randomart for a public key line (KEY-13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprints {
    /// `SHA256:…`, what OpenSSH shows by default.
    pub sha256: String,
    /// `MD5:aa:bb:…`, what older tools and some cloud consoles show.
    pub md5: String,
    /// The picture `ssh-keygen -lv` draws, as lines.
    pub randomart: String,
}

/// The fingerprints of the public key in `public_line` (`ssh-ed25519 AAAA… comment`).
///
/// # Errors
///
/// [`SshError::KeyUnreadable`] if the line isn't an OpenSSH public key.
pub fn fingerprints(public_line: &str) -> Result<Fingerprints, SshError> {
    let key = PublicKey::from_openssh(public_line.trim()).map_err(|e| SshError::KeyUnreadable {
        reason: e.to_string(),
    })?;
    let wire = key.to_bytes().map_err(|source| SshError::KeyEncoding {
        algorithm: key.algorithm().to_string(),
        source,
    })?;
    let md5 = md5::Md5::digest(&wire)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":");
    let header = match kind_of(&key) {
        Ok(kind) => randomart_header(kind),
        Err(_) => format!("[{}]", key.algorithm().as_str()),
    };
    Ok(Fingerprints {
        sha256: fingerprint(&key),
        md5: format!("MD5:{md5}"),
        randomart: key.fingerprint(HashAlg::Sha256).to_randomart(&header),
    })
}

/// `[ED25519 256]`, `[ECDSA 384]`, `[RSA 3072]`, as `ssh-keygen -lv` titles its picture.
fn randomart_header(kind: KeyKind) -> String {
    match kind {
        KeyKind::Ed25519 => "[ED25519 256]".to_owned(),
        KeyKind::Ecdsa(curve) => format!("[ECDSA {}]", curve.bits()),
        KeyKind::Rsa(bits) => format!("[RSA {bits}]"),
    }
}

fn algorithm(kind: KeyKind) -> Algorithm {
    match kind {
        KeyKind::Ed25519 => Algorithm::Ed25519,
        KeyKind::Ecdsa(EcdsaCurve::P256) => Algorithm::Ecdsa {
            curve: SshCurve::NistP256,
        },
        KeyKind::Ecdsa(EcdsaCurve::P384) => Algorithm::Ecdsa {
            curve: SshCurve::NistP384,
        },
        KeyKind::Ecdsa(EcdsaCurve::P521) => Algorithm::Ecdsa {
            curve: SshCurve::NistP521,
        },
        KeyKind::Rsa(_) => Algorithm::Rsa { hash: None },
    }
}

/// The keychain's name for `key`'s algorithm.
fn kind_of(key: &PublicKey) -> Result<KeyKind, SshError> {
    match key.algorithm() {
        Algorithm::Ed25519 => Ok(KeyKind::Ed25519),
        Algorithm::Ecdsa { curve } => Ok(KeyKind::Ecdsa(match curve {
            SshCurve::NistP256 => EcdsaCurve::P256,
            SshCurve::NistP384 => EcdsaCurve::P384,
            SshCurve::NistP521 => EcdsaCurve::P521,
        })),
        Algorithm::Rsa { .. } => key
            .key_data()
            .rsa()
            .map(|rsa| KeyKind::Rsa(rsa.key_size()))
            .ok_or_else(|| SshError::KeyUnsupported {
                algorithm: "RSA".to_owned(),
            }),
        other => Err(SshError::KeyUnsupported {
            algorithm: other.to_string(),
        }),
    }
}

fn public_line(key: &PublicKey) -> Result<String, SshError> {
    key.to_openssh().map_err(|source| SshError::KeyEncoding {
        algorithm: key.algorithm().to_string(),
        source,
    })
}

/// `key` as [`KeyMaterial`], encrypted with `passphrase` if one is given.
fn material(key: &PrivateKey, passphrase: Option<&SecretString>) -> Result<KeyMaterial, SshError> {
    let kind = kind_of(key.public_key())?;
    let export_failed = |source: ssh_key::Error| SshError::KeyExport {
        reason: source.to_string(),
    };
    let encrypted;
    let to_write = match passphrase.filter(|p| !p.expose_secret().is_empty()) {
        Some(passphrase) => {
            encrypted = key
                .encrypt(&mut rand::rngs::SysRng, passphrase.expose_secret())
                .map_err(export_failed)?;
            &encrypted
        }
        None => key,
    };
    let text = to_write.to_openssh(LineEnding::LF).map_err(export_failed)?;
    Ok(KeyMaterial {
        kind,
        public_key: public_line(key.public_key())?,
        fingerprint: fingerprint(key.public_key()),
        private_key: SecretString::from(text.as_str()),
        is_encrypted: to_write.is_encrypted(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    fn fixture(name: &str) -> SecretString {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        SecretString::from(std::fs::read_to_string(path).unwrap())
    }

    fn public_fixture(name: &str) -> String {
        fixture(&format!("{name}.pub"))
            .expose_secret()
            .trim()
            .to_owned()
    }

    fn pass(text: &str) -> SecretString {
        SecretString::from(text)
    }

    #[test]
    fn generated_keys_have_the_kind_asked_for_and_load_back() {
        let kinds = [
            KeyKind::Ed25519,
            KeyKind::Ecdsa(EcdsaCurve::P256),
            KeyKind::Ecdsa(EcdsaCurve::P384),
            KeyKind::Ecdsa(EcdsaCurve::P521),
            KeyKind::Rsa(2048),
        ];
        for kind in kinds {
            let key = generate_blocking(kind, "me@test", None).unwrap();
            assert_eq!(key.kind, kind);
            assert!(!key.is_encrypted);
            assert!(key.public_key.ends_with(" me@test"), "{}", key.public_key);
            let back = import_blocking(&key.private_key, None).unwrap();
            assert_eq!(back.public_key, key.public_key);
            assert_eq!(back.fingerprint, key.fingerprint);
            assert_eq!(back.kind, kind);
        }
    }

    #[test]
    fn a_generated_key_with_a_passphrase_is_encrypted() {
        let passphrase = pass("open sesame");
        let key = generate_blocking(KeyKind::Ed25519, "", Some(&passphrase)).unwrap();
        assert!(key.is_encrypted);
        assert!(is_encrypted(&key.private_key).unwrap());
        assert!(matches!(
            open_blocking(&key.private_key, None),
            Err(SshError::KeyNeedsPassphrase)
        ));
        assert!(matches!(
            open_blocking(&key.private_key, Some(&pass("wrong"))),
            Err(SshError::KeyWrongPassphrase)
        ));
        let opened = open_blocking(&key.private_key, Some(&passphrase)).unwrap();
        assert_eq!(fingerprint(opened.public_key()), key.fingerprint);
        // An empty passphrase means none.
        let plain = generate_blocking(KeyKind::Ed25519, "", Some(&pass(""))).unwrap();
        assert!(!plain.is_encrypted);
    }

    #[test]
    fn rsa_sizes_outside_the_range_are_refused() {
        for bits in [1024, 8192] {
            assert!(matches!(
                generate_blocking(KeyKind::Rsa(bits), "", None),
                Err(SshError::KeyUnsupported { .. })
            ));
        }
    }

    #[test]
    fn importing_each_fixture_format() {
        let cases = [
            ("id_ed25519", None, KeyKind::Ed25519, false, "id_ed25519"),
            (
                "id_ecdsa",
                None,
                KeyKind::Ecdsa(EcdsaCurve::P256),
                false,
                "id_ecdsa",
            ),
            ("id_rsa", None, KeyKind::Rsa(3072), false, "id_rsa"),
            ("id_rsa_pkcs8", None, KeyKind::Rsa(3072), false, "id_rsa"),
            // Encrypted OpenSSH: imported as it is, with or without the passphrase.
            (
                "id_ed25519_encrypted",
                None,
                KeyKind::Ed25519,
                true,
                "id_ed25519_encrypted",
            ),
            (
                "id_ed25519_encrypted",
                Some("fixture-passphrase"),
                KeyKind::Ed25519,
                true,
                "id_ed25519_encrypted",
            ),
            // Encrypted PEM: converted, and encrypted again with the same passphrase.
            (
                "id_rsa_pem_encrypted",
                Some("fixture-passphrase"),
                KeyKind::Rsa(3072),
                true,
                "id_rsa",
            ),
        ];
        for (file, passphrase, kind, encrypted, public) in cases {
            let passphrase = passphrase.map(pass);
            let key = import_blocking(&fixture(file), passphrase.as_ref())
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(key.kind, kind, "{file}");
            assert_eq!(key.is_encrypted, encrypted, "{file}");
            let expected = public_fixture(public);
            // The comment isn't kept by PEM formats; compare type and key data.
            let head = |line: &str| line.split(' ').take(2).collect::<Vec<_>>().join(" ");
            assert_eq!(head(&key.public_key), head(&expected), "{file}");
            assert!(
                key.private_key
                    .expose_secret()
                    .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
            );
            if encrypted {
                let opened = open_blocking(&key.private_key, Some(&pass("fixture-passphrase")));
                assert!(opened.is_ok(), "{file}");
            }
        }
    }

    #[test]
    fn import_failures_say_why() {
        assert!(matches!(
            import_blocking(&fixture("id_rsa_pem_encrypted"), None),
            Err(SshError::KeyNeedsPassphrase)
        ));
        assert!(matches!(
            import_blocking(&fixture("id_rsa_pem_encrypted"), Some(&pass("nope"))),
            Err(SshError::KeyWrongPassphrase)
        ));
        assert!(matches!(
            import_blocking(&fixture("id_ed25519_encrypted"), Some(&pass("nope"))),
            Err(SshError::KeyWrongPassphrase)
        ));
        for junk in ["", "hello", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA"] {
            assert!(
                matches!(
                    import_blocking(&pass(junk), None),
                    Err(SshError::KeyUnreadable { .. })
                ),
                "{junk:?}"
            );
        }
    }

    #[test]
    fn exports_round_trip() {
        let plain = import_blocking(&fixture("id_ed25519"), None).unwrap();
        let openssh = export_blocking(&plain.private_key, None, ExportFormat::OpenSsh).unwrap();
        assert_eq!(openssh.expose_secret(), plain.private_key.expose_secret());
        let pkcs8 = export_blocking(&plain.private_key, None, ExportFormat::Pkcs8).unwrap();
        assert!(
            pkcs8
                .expose_secret()
                .starts_with("-----BEGIN PRIVATE KEY-----")
        );
        let back = import_blocking(&pkcs8, None).unwrap();
        assert_eq!(back.fingerprint, plain.fingerprint);

        let encrypted = import_blocking(&fixture("id_ed25519_encrypted"), None).unwrap();
        let passphrase = pass("fixture-passphrase");
        assert!(matches!(
            export_blocking(&encrypted.private_key, None, ExportFormat::Pkcs8),
            Err(SshError::KeyNeedsPassphrase)
        ));
        let pkcs8 = export_blocking(
            &encrypted.private_key,
            Some(&passphrase),
            ExportFormat::Pkcs8,
        )
        .unwrap();
        assert!(
            pkcs8
                .expose_secret()
                .starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----")
        );
        let back = import_blocking(&pkcs8, Some(&passphrase)).unwrap();
        assert_eq!(back.fingerprint, encrypted.fingerprint);
    }

    /// Expected values from `ssh-keygen -l -E md5` and `ssh-keygen -lv` on the fixture.
    #[test]
    fn fingerprints_match_ssh_keygen() {
        let prints = fingerprints(&public_fixture("id_ed25519")).unwrap();
        assert_eq!(
            prints.sha256,
            "SHA256:iEssdgZzVEZ0tRYtUlI1D5UFmPQON21E7zMspQmTgqo"
        );
        assert_eq!(
            prints.md5,
            "MD5:f5:1a:86:53:a8:4e:84:ac:d0:bd:95:12:4f:8f:a5:40"
        );
        let expected = "\
+--[ED25519 256]--+
|    .+= o+=+=+o=+|
|   . . .o..o==.o.|
|  o .  . oo=. =.+|
|   = ... .. o+=+ |
|  o *.. S    +.+.|
| . =..        . o|
|   E.            |
|                 |
|                 |
+----[SHA256]-----+";
        assert_eq!(prints.randomart.trim_end(), expected);
        assert!(matches!(
            fingerprints("not a key"),
            Err(SshError::KeyUnreadable { .. })
        ));
    }

    #[test]
    fn debug_never_shows_the_private_key() {
        let key = import_blocking(&fixture("id_ed25519"), None).unwrap();
        let debug = format!("{key:?}");
        assert!(!debug.contains("PRIVATE"), "{debug}");
        assert!(debug.contains("[redacted]"));
    }
}
