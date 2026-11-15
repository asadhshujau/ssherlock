//! Which algorithms a connection offers, and whether it compresses (CONN-17, CONN-18).
//!
//! **Service** layer. Preferences are OpenSSH algorithm names (`aes256-gcm@openssh.com`,
//! `curve25519-sha256`), so per-host settings can store them as text and the user can type what
//! a server's documentation says. An empty list means SSHerlock's safe defaults.

use std::borrow::Cow;

use russh::keys::Algorithm;
use russh::{Preferred, cipher, compression, kex, mac};

use crate::SshError;

/// Algorithms to offer, most preferred first. Empty lists keep the defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlgorithmPreferences {
    /// Key exchange, e.g. `curve25519-sha256`.
    pub kex: Vec<String>,
    /// Host key types the server may prove itself with, e.g. `ssh-ed25519`, `rsa-sha2-512`.
    pub host_key: Vec<String>,
    /// Ciphers, e.g. `chacha20-poly1305@openssh.com`.
    pub cipher: Vec<String>,
    /// MACs, e.g. `hmac-sha2-256-etm@openssh.com` (unused with AEAD ciphers).
    pub mac: Vec<String>,
}

/// Whether to compress the connection. Worth it only on slow links; off by default, as in
/// OpenSSH.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// No compression.
    #[default]
    Off,
    /// zlib, started after login as OpenSSH does.
    On,
}

/// Kex names that aren't algorithms but tell the server what we support: server-sig-algs
/// extension info, and strict key exchange (the Terrapin fix). Always offered.
const KEX_MARKERS: [kex::Name; 2] = [
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

impl AlgorithmPreferences {
    /// Checks every name is one SSHerlock supports, so a settings form can refuse a typo
    /// before connecting.
    ///
    /// # Errors
    ///
    /// [`SshError::UnknownAlgorithm`] naming the first unsupported entry.
    pub fn validate(&self) -> Result<(), SshError> {
        self.to_preferred(Compression::Off).map(|_| ())
    }

    /// russh's preference lists for these choices.
    pub(crate) fn to_preferred(&self, compression: Compression) -> Result<Preferred, SshError> {
        let mut preferred = Preferred::default();
        if !self.kex.is_empty() {
            // russh's name lookup doesn't know the markers, so a list that names them is
            // matched here; they're moved to the end below either way.
            let mut names = parse("key exchange", &self.kex, |n| {
                KEX_MARKERS
                    .into_iter()
                    .find(|marker| marker.as_ref() == n)
                    .or_else(|| kex::Name::try_from(n).ok())
            })?;
            names.retain(|name| !KEX_MARKERS.contains(name));
            names.extend(KEX_MARKERS);
            preferred.kex = Cow::Owned(names);
        }
        if !self.host_key.is_empty() {
            preferred.key = Cow::Owned(parse("host key", &self.host_key, |n| {
                Algorithm::new(n).ok()
            })?);
        }
        if !self.cipher.is_empty() {
            preferred.cipher = Cow::Owned(parse("cipher", &self.cipher, |n| {
                cipher::Name::try_from(n).ok()
            })?);
        }
        if !self.mac.is_empty() {
            preferred.mac = Cow::Owned(parse("MAC", &self.mac, |n| mac::Name::try_from(n).ok())?);
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
        let preferred = AlgorithmPreferences::default()
            .to_preferred(Compression::Off)
            .unwrap();
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
        let preferred = prefs.to_preferred(Compression::On).unwrap();
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
        let preferred = prefs.to_preferred(Compression::Off).unwrap();
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
            match prefs.validate() {
                Err(SshError::UnknownAlgorithm { kind: k, name: n }) => {
                    assert_eq!((k, n.as_str()), (kind, name));
                }
                other => panic!("expected UnknownAlgorithm for {name}, got {other:?}"),
            }
        }
    }
}
