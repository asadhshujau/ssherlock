//! A saved SSH key (KEY-01).

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{LabelError, check_label};
use crate::id::Id;
use crate::secret::Sealed;

/// An SSH key pair kept in the keychain.
///
/// The public half and its fingerprint are plain text, so the keychain list can show them
/// without the vault. The private half is the OpenSSH key file's text, sealed; when the key has
/// its own passphrase, the file stays encrypted with it and [`KeyProtection`] says whether that
/// passphrase is saved too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// Unique id.
    pub id: Id<Key>,
    /// The name shown in the keychain. Empty means "show the fingerprint".
    #[serde(default)]
    pub label: String,
    /// The key's algorithm and size.
    pub kind: KeyKind,
    /// The public key as one OpenSSH line: `ssh-ed25519 AAAA… comment`.
    pub public_key: String,
    /// The SHA-256 fingerprint, `SHA256:…`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
    /// The private key file's text (OpenSSH format), sealed by the vault.
    pub private_key: Sealed,
    /// Whether the private key needs a passphrase, and whether it's saved.
    pub protection: KeyProtection,
    /// When the key was added.
    pub created_at: Timestamp,
    /// When the key was last edited.
    pub updated_at: Timestamp,
}

/// Whether a key's private half is encrypted with its own passphrase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyProtection {
    /// No passphrase; the vault's sealing is the only protection.
    Unencrypted,
    /// Encrypted; the passphrase is asked for when the key is used, then remembered for a while
    /// (KEY-11).
    AskPassphrase,
    /// Encrypted, with the passphrase saved in the vault too.
    SavedPassphrase(Sealed),
}

impl KeyProtection {
    /// Whether the private key is encrypted with a passphrase.
    pub fn is_encrypted(&self) -> bool {
        !matches!(self, Self::Unencrypted)
    }
}

/// A key's algorithm, with its size where the algorithm has a choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyKind {
    /// Ed25519, the default for new keys.
    Ed25519,
    /// ECDSA on a NIST curve.
    Ecdsa(EcdsaCurve),
    /// RSA with a modulus of this many bits.
    Rsa(u32),
}

/// The NIST curves ECDSA keys use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EcdsaCurve {
    /// P-256.
    P256,
    /// P-384.
    P384,
    /// P-521.
    P521,
}

impl EcdsaCurve {
    /// The curve's size in bits.
    pub fn bits(self) -> u32 {
        match self {
            Self::P256 => 256,
            Self::P384 => 384,
            Self::P521 => 521,
        }
    }
}

impl KeyKind {
    /// The smallest RSA key SSHerlock generates (KEY-04).
    pub const RSA_MIN_BITS: u32 = 2048;
    /// The largest RSA key SSHerlock generates.
    pub const RSA_MAX_BITS: u32 = 4096;

    /// The choices offered when generating a key, default first.
    pub const GENERATE_CHOICES: [Self; 6] = [
        Self::Ed25519,
        Self::Ecdsa(EcdsaCurve::P256),
        Self::Ecdsa(EcdsaCurve::P384),
        Self::Ecdsa(EcdsaCurve::P521),
        Self::Rsa(3072),
        Self::Rsa(4096),
    ];

    /// A short name for lists: `Ed25519`, `ECDSA P-256`, `RSA 4096`.
    pub fn display_name(self) -> String {
        match self {
            Self::Ed25519 => "Ed25519".to_owned(),
            Self::Ecdsa(curve) => format!("ECDSA P-{}", curve.bits()),
            Self::Rsa(bits) => format!("RSA {bits}"),
        }
    }
}

/// Why text isn't a [`KeyKind`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a key type SSHerlock knows")]
pub struct KeyKindError(String);

/// Stored as `ed25519`, `ecdsa-p256`, `rsa-4096`.
impl fmt::Display for KeyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ed25519 => f.write_str("ed25519"),
            Self::Ecdsa(curve) => write!(f, "ecdsa-p{}", curve.bits()),
            Self::Rsa(bits) => write!(f, "rsa-{bits}"),
        }
    }
}

impl FromStr for KeyKind {
    type Err = KeyKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || KeyKindError(text.to_owned());
        match text {
            "ed25519" => Ok(Self::Ed25519),
            "ecdsa-p256" => Ok(Self::Ecdsa(EcdsaCurve::P256)),
            "ecdsa-p384" => Ok(Self::Ecdsa(EcdsaCurve::P384)),
            "ecdsa-p521" => Ok(Self::Ecdsa(EcdsaCurve::P521)),
            _ => {
                let bits = text.strip_prefix("rsa-").ok_or_else(error)?;
                bits.parse().map(Self::Rsa).map_err(|_| error())
            }
        }
    }
}

impl Serialize for KeyKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KeyKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Why a [`Key`] can't be saved.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The label is longer than 120 characters.
    #[error("the label is longer than 120 characters")]
    LabelTooLong,
    /// The label has a newline or other control character.
    #[error("the label cannot contain line breaks or control characters")]
    LabelControl,
    /// The public key is empty or isn't one line.
    #[error("the public key must be a single line in OpenSSH format")]
    PublicKeyShape,
}

impl Key {
    /// The name to show: the label, or the fingerprint when there's no label.
    pub fn title(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() {
            &self.fingerprint
        } else {
            label
        }
    }

    /// The comment at the end of the public key line, if any (often `user@machine`).
    pub fn comment(&self) -> Option<&str> {
        let mut parts = self.public_key.splitn(3, ' ');
        parts.nth(2).map(str::trim).filter(|c| !c.is_empty())
    }

    /// Checks the key can be saved. Pure and total.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), KeyError> {
        check_label(&self.label).map_err(|e| match e {
            LabelError::TooLong => KeyError::LabelTooLong,
            LabelError::Control => KeyError::LabelControl,
        })?;
        let public = self.public_key.trim();
        if public.is_empty() || public.contains(['\n', '\r']) || !public.contains(' ') {
            return Err(KeyError::PublicKeyShape);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    /// A key with made-up contents, for tests across the crate.
    pub(crate) fn key(label: &str) -> Key {
        Key {
            id: Id::generate(),
            label: label.into(),
            kind: KeyKind::Ed25519,
            public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA tester@example".into(),
            fingerprint: "SHA256:abc".into(),
            private_key: Sealed::from_bytes(vec![1, 2, 3]),
            protection: KeyProtection::Unencrypted,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn kind_text_round_trips_and_rejects_the_unknown() {
        let cases = [
            (KeyKind::Ed25519, "ed25519", "Ed25519"),
            (
                KeyKind::Ecdsa(EcdsaCurve::P256),
                "ecdsa-p256",
                "ECDSA P-256",
            ),
            (
                KeyKind::Ecdsa(EcdsaCurve::P384),
                "ecdsa-p384",
                "ECDSA P-384",
            ),
            (
                KeyKind::Ecdsa(EcdsaCurve::P521),
                "ecdsa-p521",
                "ECDSA P-521",
            ),
            (KeyKind::Rsa(4096), "rsa-4096", "RSA 4096"),
        ];
        for (kind, text, name) in cases {
            assert_eq!(kind.to_string(), text);
            assert_eq!(text.parse::<KeyKind>().unwrap(), kind);
            assert_eq!(kind.display_name(), name);
        }
        for bad in ["", "dsa", "rsa-", "rsa-big", "ecdsa-p192", "ED25519"] {
            assert!(bad.parse::<KeyKind>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn validation() {
        let with = |label: &str, public: &str| Key {
            label: label.into(),
            public_key: public.into(),
            ..key("")
        };
        let good = "ssh-ed25519 AAAA";
        let cases = [
            (with("", good), Ok(())),
            (with("Work laptop", good), Ok(())),
            (with(&"k".repeat(121), good), Err(KeyError::LabelTooLong)),
            (with("a\tb", good), Err(KeyError::LabelControl)),
            (with("", ""), Err(KeyError::PublicKeyShape)),
            (with("", "AAAA"), Err(KeyError::PublicKeyShape)),
            (
                with("", "ssh-ed25519 AAAA\nssh-rsa B"),
                Err(KeyError::PublicKeyShape),
            ),
        ];
        for (k, expected) in cases {
            assert_eq!(k.validate(), expected, "{:?} {:?}", k.label, k.public_key);
        }
    }

    #[test]
    fn title_and_comment() {
        let mut k = key("");
        assert_eq!(k.title(), "SHA256:abc");
        assert_eq!(k.comment(), Some("tester@example"));
        k.label = " Deploy ".into();
        k.public_key = "ssh-ed25519 AAAA".into();
        assert_eq!(k.title(), "Deploy");
        assert_eq!(k.comment(), None);
    }

    #[test]
    fn json_never_holds_plaintext_and_round_trips() {
        let mut k = key("x");
        k.protection = KeyProtection::SavedPassphrase(Sealed::from_bytes(vec![9; 4]));
        let json = serde_json::to_string(&k).unwrap();
        assert!(json.contains(r#""kind":"ed25519""#), "{json}");
        assert_eq!(serde_json::from_str::<Key>(&json).unwrap(), k);
        assert!(k.protection.is_encrypted());
        assert!(!KeyProtection::Unencrypted.is_encrypted());
    }
}
