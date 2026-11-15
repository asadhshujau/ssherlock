//! Host key trust across several `known_hosts` files: one SSHerlock writes, others it only reads.
//!
//! **Service** layer. Files are re-read on every check, so changes made by other tools (or other
//! SSHerlock windows) are seen immediately; writes are serialised so two sessions trusting keys
//! at once can't lose each other's entries.

use std::path::PathBuf;
use std::sync::Arc;

use russh::keys::PublicKey;

use crate::SshError;
use crate::known_hosts::{HostHashing, HostKeyStatus, KnownHostName, KnownHosts};

/// Where host keys are trusted. Cheap to clone; clones share the write lock.
#[derive(Clone, Debug)]
pub struct HostKeys {
    writable: PathBuf,
    read_only: Vec<PathBuf>,
    hashing: HostHashing,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl HostKeys {
    /// Trust store that saves new keys to `writable` and also honours keys in `read_only`
    /// (typically `~/.ssh/known_hosts`, so hosts already trusted with OpenSSH just work).
    pub fn new(
        writable: impl Into<PathBuf>,
        read_only: Vec<PathBuf>,
        hashing: HostHashing,
    ) -> Self {
        Self {
            writable: writable.into(),
            read_only,
            hashing,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Checks `key` for `name` against every file, with OpenSSH's precedence: revoked anywhere,
    /// then trusted anywhere, then changed.
    ///
    /// # Errors
    ///
    /// [`SshError::KnownHostsRead`] if a file exists but can't be read.
    pub async fn check(
        &self,
        name: &KnownHostName,
        key: &PublicKey,
    ) -> Result<HostKeyStatus, SshError> {
        let mut statuses = Vec::with_capacity(1 + self.read_only.len());
        for path in std::iter::once(&self.writable).chain(&self.read_only) {
            statuses.push(KnownHosts::load(path).await?.check(name, key));
        }
        Ok(combine(statuses))
    }

    /// Records that the user trusts `key` for `name`, replacing any stale key of the same
    /// algorithm in the writable file.
    ///
    /// # Errors
    ///
    /// [`SshError::KnownHostsRead`] / [`SshError::KnownHostsWrite`] if the writable file can't
    /// be read or written, and the errors of [`KnownHosts::replace`].
    pub async fn trust(&self, name: &KnownHostName, key: &PublicKey) -> Result<(), SshError> {
        let _guard = self.write_lock.lock().await;
        let mut known = KnownHosts::load(&self.writable).await?;
        let removed = known.replace(name, key, self.hashing)?;
        known.save().await?;
        tracing::info!(
            host = name.as_str(),
            algorithm = %key.algorithm(),
            replaced = removed,
            path = %self.writable.display(),
            "trusted host key"
        );
        Ok(())
    }
}

/// Merges per-file results: any revoked wins, then any trusted, then changed (with every
/// conflicting key), then unknown (with every other-algorithm key).
fn combine(statuses: Vec<HostKeyStatus>) -> HostKeyStatus {
    let mut changed = Vec::new();
    let mut other_algorithms = Vec::new();
    let mut trusted = false;
    for status in statuses {
        match status {
            revoked @ HostKeyStatus::Revoked { .. } => return revoked,
            HostKeyStatus::Trusted => trusted = true,
            HostKeyStatus::Changed { known } => changed.extend(known),
            HostKeyStatus::Unknown {
                other_algorithms: other,
            } => other_algorithms.extend(other),
        }
    }
    if trusted {
        HostKeyStatus::Trusted
    } else if changed.is_empty() {
        HostKeyStatus::Unknown { other_algorithms }
    } else {
        HostKeyStatus::Changed { known: changed }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::known_hosts::{KnownKey, KnownKeyLocation};

    const ED25519: &str = include_str!("../tests/fixtures/id_ed25519.pub");
    const OTHER: &str = include_str!("../tests/fixtures/id_unauthorized.pub");

    fn key(line: &str) -> PublicKey {
        PublicKey::from_openssh(line.trim()).unwrap()
    }

    fn known(line: usize) -> KnownKey {
        KnownKey {
            at: KnownKeyLocation {
                path: PathBuf::from("/kh"),
                line,
            },
            key: key(OTHER),
        }
    }

    fn revoked() -> HostKeyStatus {
        HostKeyStatus::Revoked {
            at: KnownKeyLocation {
                path: PathBuf::from("/kh"),
                line: 9,
            },
        }
    }

    fn unknown(other: Vec<KnownKey>) -> HostKeyStatus {
        HostKeyStatus::Unknown {
            other_algorithms: other,
        }
    }

    #[test]
    fn combining() {
        use HostKeyStatus::{Changed, Trusted};
        let cases = vec![
            (vec![], unknown(vec![])),
            (vec![unknown(vec![]), unknown(vec![])], unknown(vec![])),
            (
                vec![unknown(vec![known(1)]), unknown(vec![known(2)])],
                unknown(vec![known(1), known(2)]),
            ),
            (
                vec![
                    Changed {
                        known: vec![known(1)],
                    },
                    Trusted,
                ],
                Trusted,
            ),
            (vec![Trusted, revoked()], revoked()),
            (
                vec![
                    Changed {
                        known: vec![known(1)],
                    },
                    unknown(vec![known(3)]),
                    Changed {
                        known: vec![known(2)],
                    },
                ],
                Changed {
                    known: vec![known(1), known(2)],
                },
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(combine(input.clone()), expected, "{input:?}");
        }
    }

    #[tokio::test]
    async fn trust_then_check_across_files() {
        let dir = std::env::temp_dir().join(format!("ssherlock-hostkeys-{}", std::process::id()));
        let writable = dir.join("known_hosts");
        let openssh = dir.join("ssh_known_hosts");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let name = KnownHostName::new("example.com", 22);
        tokio::fs::write(&openssh, format!("example.com {}", OTHER.trim()))
            .await
            .unwrap();

        let keys = HostKeys::new(&writable, vec![openssh.clone()], HostHashing::Hashed);
        let status = keys.check(&name, &key(ED25519)).await.unwrap();
        assert!(
            matches!(status, HostKeyStatus::Changed { .. }),
            "{status:?}"
        );

        keys.trust(&name, &key(ED25519)).await.unwrap();
        assert_eq!(
            keys.check(&name, &key(ED25519)).await.unwrap(),
            HostKeyStatus::Trusted
        );
        // The read-only file is never touched.
        let untouched = tokio::fs::read_to_string(&openssh).await.unwrap();
        assert_eq!(untouched, format!("example.com {}", OTHER.trim()));

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
