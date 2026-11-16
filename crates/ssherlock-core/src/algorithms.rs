//! Which algorithms a connection offers, and whether it compresses (CONN-17, CONN-18).
//!
//! **Model** layer: the plain choices, so saved hosts and groups can store them. Checking the
//! names and turning them into russh's lists happens in `ssherlock-ssh::algorithms`.

use serde::{Deserialize, Serialize};

/// Algorithms to offer, most preferred first, as OpenSSH names (`aes256-gcm@openssh.com`,
/// `curve25519-sha256`) so the user can type what a server's documentation says. An empty list
/// means SSHerlock's safe defaults (or, in a saved host's settings, "inherit from the group").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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

impl AlgorithmPreferences {
    /// Whether every list is empty, i.e. nothing is chosen.
    pub fn is_empty(&self) -> bool {
        self.kex.is_empty()
            && self.host_key.is_empty()
            && self.cipher.is_empty()
            && self.mac.is_empty()
    }
}

/// Whether to compress the connection. Worth it only on slow links; off by default, as in
/// OpenSSH.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    /// No compression.
    #[default]
    Off,
    /// zlib, started after login as OpenSSH does.
    On,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn empty_means_nothing_chosen() {
        assert!(AlgorithmPreferences::default().is_empty());
        let cipher_only = AlgorithmPreferences {
            cipher: vec!["aes128-ctr".into()],
            ..AlgorithmPreferences::default()
        };
        assert!(!cipher_only.is_empty());
    }

    #[test]
    fn serde_fills_missing_lists() {
        let parsed: AlgorithmPreferences =
            serde_json::from_str(r#"{"cipher":["aes128-ctr"]}"#).unwrap();
        assert_eq!(parsed.cipher, ["aes128-ctr"]);
        assert_eq!(parsed.kex.len(), 0);
        assert_eq!(serde_json::to_string(&Compression::On).unwrap(), r#""on""#);
    }
}
