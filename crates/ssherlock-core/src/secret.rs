//! Types for holding secrets: passwords, passphrases, private keys and tokens.
//!
//! Re-exported from [`secrecy`] so every crate names them the same way. A secret is read only
//! through [`ExposeSecret::expose_secret`], which makes each read easy to find in review; its
//! `Debug` output is redacted and its memory is zeroed on drop.
//!
//! A secret that is saved becomes a [`Sealed`] value: ciphertext made by the vault in
//! `ssherlock-store`. Models hold `Sealed` values, never plaintext, so they can be listed,
//! copied and written to the database without exposing anything.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
pub use secrecy::{ExposeSecret, SecretBox, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A secret encrypted by the vault. Only the vault that sealed it can open it again.
///
/// Holding one exposes nothing, so it can be cloned, compared and stored freely. `Debug` shows
/// only its length; serde writes it as base64 text, for JSON columns such as host settings.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed(Vec<u8>);

impl Sealed {
    /// Wraps ciphertext the vault made, or read back from the database.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The ciphertext, for storing.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sealed({} bytes)", self.0.len())
    }
}

impl Serialize for Sealed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Sealed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn debug_output_never_contains_the_secret() {
        let secret = SecretString::from("hunter2-correct-horse");
        let bytes: SecretBox<[u8]> = SecretBox::new(Box::from(&b"hunter2-correct-horse"[..]));

        for rendered in [
            format!("{secret:?}"),
            format!("{secret:#?}"),
            format!("{bytes:?}"),
        ] {
            assert!(
                !rendered.contains("hunter2"),
                "secret leaked through Debug: {rendered}"
            );
        }
    }

    #[test]
    fn expose_secret_returns_the_value() {
        let secret = SecretString::from("s3cret");
        assert_eq!(secret.expose_secret(), "s3cret");
    }

    #[test]
    fn sealed_debug_shows_only_the_length() {
        let sealed = Sealed::from_bytes(b"ciphertext".to_vec());
        assert_eq!(format!("{sealed:?}"), "Sealed(10 bytes)");
    }

    #[test]
    fn sealed_round_trips_through_json_as_base64() {
        let sealed = Sealed::from_bytes(vec![0, 1, 2, 250, 255]);
        let json = serde_json::to_string(&sealed).unwrap();
        assert_eq!(json, r#""AAEC+v8=""#);
        assert_eq!(serde_json::from_str::<Sealed>(&json).unwrap(), sealed);
        assert!(serde_json::from_str::<Sealed>(r#""not base64!""#).is_err());
    }
}
