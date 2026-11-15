//! What the host key and login dialogs say. Pure functions, tested without gpui; the remote
//! terminal turns them into [`Modal`](crate::components::Modal)s.

use ssherlock_ssh::PublicKey;
use ssherlock_ssh::auth::AuthPromptKind;
use ssherlock_ssh::known_hosts::{KnownHostName, KnownKey, fingerprint};
use ssherlock_ssh::session::HostKeyConcern;

/// The words of a host key dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyText {
    /// Dialog title.
    pub title: String,
    /// The explanation under it.
    pub message: String,
    /// The fingerprints to compare.
    pub rows: Vec<KeyRow>,
    /// The confirm button's label.
    pub confirm: String,
    /// A changed key: red button, and Enter doesn't accept.
    pub is_danger: bool,
}

/// One fingerprint in a host key dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    /// What it is: `Fingerprint`, `Expected`, `Received`, `Also trusted`.
    pub label: String,
    /// `SHA256:… (ssh-ed25519)`.
    pub value: String,
    /// For a key already trusted: the file and line it's in.
    pub note: Option<String>,
}

impl KeyRow {
    fn presented(label: &str, key: &PublicKey) -> Self {
        Self {
            label: label.to_owned(),
            value: format!("{} ({})", fingerprint(key), key.algorithm().as_str()),
            note: None,
        }
    }

    fn known(label: &str, known: &KnownKey) -> Self {
        Self {
            note: Some(format!(
                "{}, line {}",
                known.at.path.display(),
                known.at.line
            )),
            ..Self::presented(label, &known.key)
        }
    }
}

/// The dialog for a host key the user must decide about.
pub fn host_key_text(
    host: &KnownHostName,
    key: &PublicKey,
    concern: &HostKeyConcern,
) -> HostKeyText {
    match concern {
        HostKeyConcern::New { other_algorithms } => {
            let mut message = format!(
                "SSHerlock hasn't connected to {} before. Before trusting it, check that the \
                 fingerprint below matches the one the server's administrator gave you.",
                host.as_str()
            );
            if !other_algorithms.is_empty() {
                message.push_str(
                    " This host is already trusted with a key of another type, so the server may \
                     have added a key.",
                );
            }
            let mut rows = vec![KeyRow::presented("Fingerprint", key)];
            rows.extend(
                other_algorithms
                    .iter()
                    .map(|known| KeyRow::known("Also trusted", known)),
            );
            HostKeyText {
                title: "Unknown host".to_owned(),
                message,
                rows,
                confirm: "Trust and connect".to_owned(),
                is_danger: false,
            }
        }
        HostKeyConcern::Changed { known } => {
            let mut rows: Vec<KeyRow> = known
                .iter()
                .map(|known| KeyRow::known("Expected", known))
                .collect();
            rows.push(KeyRow::presented("Received", key));
            HostKeyText {
                title: "Warning: the host key has changed".to_owned(),
                message: format!(
                    "The key {} sent is not the one you trusted before. Someone may be \
                     intercepting this connection (a man-in-the-middle attack), or the server \
                     was reinstalled. Don't continue unless you know why the key changed.",
                    host.as_str()
                ),
                rows,
                confirm: "Trust the new key".to_owned(),
                is_danger: true,
            }
        }
    }
}

/// The words of a login prompt: title, message and confirm label.
pub fn login_text(kind: &AuthPromptKind) -> (String, String, String) {
    const RETRY: &str = " That wasn't accepted; try again.";
    let retry = |is_retry: bool| if is_retry { RETRY } else { "" };
    match kind {
        AuthPromptKind::Password {
            user,
            host,
            is_retry,
        } => (
            "Password".to_owned(),
            format!("Password for {user}@{host}.{}", retry(*is_retry)),
            "Log in".to_owned(),
        ),
        AuthPromptKind::Passphrase { key, is_retry } => (
            "Key passphrase".to_owned(),
            format!(
                "The key {} is encrypted.{}",
                key.display(),
                retry(*is_retry)
            ),
            "Unlock".to_owned(),
        ),
        AuthPromptKind::Server { name, instructions } => (
            if name.trim().is_empty() {
                "The server asks".to_owned()
            } else {
                name.trim().to_owned()
            },
            if instructions.trim().is_empty() {
                "Answer to continue logging in.".to_owned()
            } else {
                instructions.trim().to_owned()
            },
            "Continue".to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use ssherlock_ssh::known_hosts::KnownKeyLocation;

    use super::*;

    const ED25519: &str = include_str!("../../../ssherlock-ssh/tests/fixtures/id_ed25519.pub");
    const OTHER: &str = include_str!("../../../ssherlock-ssh/tests/fixtures/id_ecdsa.pub");

    fn key(line: &str) -> PublicKey {
        PublicKey::from_openssh(line.trim()).unwrap()
    }

    fn known(line: &str, at: usize) -> KnownKey {
        KnownKey {
            at: KnownKeyLocation {
                path: PathBuf::from("/kh"),
                line: at,
            },
            key: key(line),
        }
    }

    #[test]
    fn new_host() {
        let host = KnownHostName::new("example.com", 2222);
        let concern = HostKeyConcern::New {
            other_algorithms: vec![],
        };
        let text = host_key_text(&host, &key(ED25519), &concern);
        assert_eq!(text.title, "Unknown host");
        assert!(
            text.message.contains("[example.com]:2222"),
            "{}",
            text.message
        );
        assert!(!text.is_danger);
        assert_eq!(text.rows.len(), 1);
        assert_eq!(text.rows[0].label, "Fingerprint");
        assert!(text.rows[0].value.starts_with("SHA256:"));
        assert!(text.rows[0].value.ends_with(" (ssh-ed25519)"));
        assert_eq!(text.rows[0].note, None);
    }

    #[test]
    fn new_host_with_another_key_type_known() {
        let host = KnownHostName::new("example.com", 22);
        let concern = HostKeyConcern::New {
            other_algorithms: vec![known(OTHER, 3)],
        };
        let text = host_key_text(&host, &key(ED25519), &concern);
        assert!(text.message.contains("another type"), "{}", text.message);
        assert_eq!(text.rows.len(), 2);
        assert!(text.rows[1].value.ends_with("(ecdsa-sha2-nistp256)"));
        assert_eq!(text.rows[1].note.as_deref(), Some("/kh, line 3"));
    }

    #[test]
    fn changed_key_is_a_danger_dialog_showing_both_fingerprints() {
        let host = KnownHostName::new("example.com", 22);
        let old = known(OTHER, 7);
        let concern = HostKeyConcern::Changed {
            known: vec![old.clone()],
        };
        let text = host_key_text(&host, &key(ED25519), &concern);
        assert!(text.is_danger);
        assert!(text.title.contains("changed"));
        assert!(text.message.contains("man-in-the-middle"));
        assert_eq!(text.rows[0].label, "Expected");
        assert!(text.rows[0].value.starts_with(&old.fingerprint()));
        assert_eq!(text.rows[0].note.as_deref(), Some("/kh, line 7"));
        assert_eq!(text.rows[1].label, "Received");
        assert_eq!(text.rows[1].note, None);
        assert_ne!(text.rows[0].value, text.rows[1].value);
    }

    #[test]
    fn login_prompts() {
        let password = |is_retry| AuthPromptKind::Password {
            user: "u".into(),
            host: "h".into(),
            is_retry,
        };
        let cases = [
            (password(false), ("Password", "Password for u@h.", "Log in")),
            (
                password(true),
                (
                    "Password",
                    "Password for u@h. That wasn't accepted; try again.",
                    "Log in",
                ),
            ),
            (
                AuthPromptKind::Passphrase {
                    key: PathBuf::from("/k/id"),
                    is_retry: false,
                },
                ("Key passphrase", "The key /k/id is encrypted.", "Unlock"),
            ),
            (
                AuthPromptKind::Server {
                    name: String::new(),
                    instructions: String::new(),
                },
                (
                    "The server asks",
                    "Answer to continue logging in.",
                    "Continue",
                ),
            ),
            (
                AuthPromptKind::Server {
                    name: " Duo ".into(),
                    instructions: "Enter code\n".into(),
                },
                ("Duo", "Enter code", "Continue"),
            ),
        ];
        for (kind, (title, message, confirm)) in cases {
            let got = login_text(&kind);
            assert_eq!(
                (got.0.as_str(), got.1.as_str(), got.2.as_str()),
                (title, message, confirm),
                "{kind:?}"
            );
        }
    }
}
