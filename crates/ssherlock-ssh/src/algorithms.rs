//! Which algorithms a connection offers, and whether it compresses (CONN-17, CONN-18).
//!
//! **Service** layer. The choices themselves ([`AlgorithmPreferences`], [`Compression`]) are
//! model types in `ssherlock-core`, so saved hosts can store them; this module checks the names
//! and turns them into russh's preference lists. An empty list means SSHerlock's safe defaults.

use std::borrow::Cow;

use russh::keys::Algorithm;
use russh::{Preferred, cipher, compression, kex, mac};
pub use ssherlock_core::algorithms::{AlgorithmPreferences, Compression};

use crate::SshError;

/// Kex names that aren't algorithms but tell the server what we support: server-sig-algs
/// extension info, and strict key exchange (the Terrapin fix). Always offered.
const KEX_MARKERS: [kex::Name; 2] = [
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

/// Checks every name in `preferences` is one SSHerlock supports, so a settings form can refuse a
/// typo before connecting.
///
/// # Errors
///
/// [`SshError::UnknownAlgorithm`] naming the first unsupported entry.
pub fn validate(preferences: &AlgorithmPreferences) -> Result<(), SshError> {
    to_preferred(preferences, Compression::Off).map(|_| ())
}

/// russh's preference lists for these choices.
pub(crate) fn to_preferred(
    preferences: &AlgorithmPreferences,
    compression: Compression,
) -> Result<Preferred, SshError> {
    let mut preferred = Preferred::default();
    if !preferences.kex.is_empty() {
        // russh's name lookup doesn't know the markers, so a list that names them is
        // matched here; they're moved to the end below either way.
        let mut names = parse("key exchange", &preferences.kex, |n| {
            KEX_MARKERS
                .into_iter()
                .find(|marker| marker.as_ref() == n)
                .or_else(|| kex::Name::try_from(n).ok())
        })?;
        names.retain(|name| !KEX_MARKERS.contains(name));
        names.extend(KEX_MARKERS);
        preferred.kex = Cow::Owned(names);
    }
    if !preferences.host_key.is_empty() {
        preferred.key = Cow::Owned(parse("host key", &preferences.host_key, |n| {
            Algorithm::new(n).ok()
        })?);
    }
    if !preferences.cipher.is_empty() {
        preferred.cipher = Cow::Owned(parse("cipher", &preferences.cipher, |n| {
            cipher::Name::try_from(n).ok()
        })?);
    }
    if !preferences.mac.is_empty() {
        preferred.mac = Cow::Owned(parse("MAC", &preferences.mac, |n| {
            mac::Name::try_from(n).ok()
        })?);
    }
    preferred.compression = Cow::Borrowed(match compression {
        Compression::Off => &[compression::NONE],
        // zlib@openssh.com first: it only starts after login, so nothing about the
        // credentials is compressed (no CRIME-style length leaks).
        Compression::On => &[
            compression::ZLIB_LEGACY,
            compression::ZLIB,
            compression::NONE,
        ],
    });
    Ok(preferred)
}

fn parse<T>(
    kind: &'static str,
    names: &[String],
    lookup: impl Fn(&str) -> Option<T>,
) -> Result<Vec<T>, SshError> {
    names
        .iter()
        .map(|name| {
            let name = name.trim();
            lookup(name).ok_or_else(|| SshError::UnknownAlgorithm {
                kind,
                name: name.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use pretty_assertions::assert_eq;
    use russh::keys::HashAlg;

    use super::*;

    fn names<T: AsRef<str>>(list: &[T]) -> Vec<&str> {
        list.iter().map(AsRef::as_ref).collect()
    }

    #[test]
    fn empty_preferences_keep_the_defaults_without_compression() {
        let preferred = to_preferred(&AlgorithmPreferences::default(), Compression::Off).unwrap();
        let default = Preferred::default();
        assert_eq!(names(&preferred.kex), names(&default.kex));
        assert_eq!(preferred.key, default.key);
        assert_eq!(names(&preferred.cipher), names(&default.cipher));
        assert_eq!(names(&preferred.compression), ["none"]);
    }

    #[test]
    fn chosen_algorithms_come_first_in_order() {
        let prefs = AlgorithmPreferences {
            kex: vec![
                "diffie-hellman-group14-sha256".into(),
                " curve25519-sha256 ".into(),
            ],
            host_key: vec!["rsa-sha2-512".into(), "ssh-ed25519".into()],
            cipher: vec!["aes128-ctr".into()],
            mac: vec!["hmac-sha2-256".into()],
        };
        let preferred = to_preferred(&prefs, Compression::On).unwrap();
        assert_eq!(
            names(&preferred.kex),
            [
                "diffie-hellman-group14-sha256",
                "curve25519-sha256",
                "ext-info-c",
                "kex-strict-c-v00@openssh.com"
            ]
        );
        assert_eq!(
            *preferred.key,
            [
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512)
                },
                Algorithm::Ed25519
            ]
        );
        assert_eq!(names(&preferred.cipher), ["aes128-ctr"]);
        assert_eq!(names(&preferred.mac), ["hmac-sha2-256"]);
        assert_eq!(
            names(&preferred.compression),
            ["zlib@openssh.com", "zlib", "none"]
        );
    }

    #[test]
    fn markers_are_not_duplicated() {
        let prefs = AlgorithmPreferences {
            kex: vec!["ext-info-c".into(), "curve25519-sha256".into()],
            ..AlgorithmPreferences::default()
        };
        let preferred = to_preferred(&prefs, Compression::Off).unwrap();
        assert_eq!(
            names(&preferred.kex),
            [
                "curve25519-sha256",
                "ext-info-c",
                "kex-strict-c-v00@openssh.com"
            ]
        );
    }

    #[test]
    fn unknown_names_are_refused() {
        for (prefs, kind, name) in [
            (
                AlgorithmPreferences {
                    kex: vec!["curve9".into()],
                    ..Default::default()
                },
                "key exchange",
                "curve9",
            ),
            (
                AlgorithmPreferences {
                    host_key: vec!["ssh-dss2".into()],
                    ..Default::default()
                },
                "host key",
                "ssh-dss2",
            ),
            (
                AlgorithmPreferences {
                    cipher: vec!["rot13".into()],
                    ..Default::default()
                },
                "cipher",
                "rot13",
            ),
            (
                AlgorithmPreferences {
                    mac: vec!["crc32".into()],
                    ..Default::default()
                },
                "MAC",
                "crc32",
            ),
        ] {
            match validate(&prefs) {
                Err(SshError::UnknownAlgorithm { kind: k, name: n }) => {
                    assert_eq!((k, n.as_str()), (kind, name));
                }
                other => panic!("expected UnknownAlgorithm for {name}, got {other:?}"),
            }
        }
    }
}
