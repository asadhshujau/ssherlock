//! SSH connections on screen: the **view** layer of the SSH feature.
//!
//! [`RemoteTerminal`] shows a remote shell with its host key and login dialogs and a status bar;
//! [`QuickConnectBar`] turns typed `user@host:port` into a connection. The pane hosts both.
//! Connecting, logging in and reconnecting all happen in `ssherlock-ssh`.

pub mod actions;
pub mod dialogs;
pub mod quick_connect;
pub mod remote_terminal;
pub mod status;

use std::path::PathBuf;

use gpui::{App, Global};
pub use quick_connect::{QuickConnectBar, QuickConnectEvent};
pub use remote_terminal::RemoteTerminal;
use ssherlock_core::keychain::{Key, KeyProtection};
use ssherlock_core::{AlgorithmPreferences, Compression, Id, Sealed, SecretString};
use ssherlock_ssh::{PassphraseCache, StoredKey};
use ssherlock_store::{StoreError, Vault};

/// Where SSH finds and keeps things on this machine. Set by `main` from the platform
/// directories; tests point it at temporary ones. Without it, remote tabs explain they can't
/// connect.
#[derive(Clone, Debug)]
pub struct SshEnvironment {
    /// SSHerlock's own `known_hosts`, where newly trusted keys are saved.
    pub known_hosts: PathBuf,
    /// Other `known_hosts` files honoured but never written (e.g. `~/.ssh/known_hosts`).
    pub read_only_known_hosts: Vec<PathBuf>,
    /// `~/.ssh`, for the default key files; `None` if there's no home directory.
    pub ssh_dir: Option<PathBuf>,
    /// The local user name, used when the quick connect text has no `user@`.
    pub local_user: Option<String>,
}

impl Global for SshEnvironment {}

/// How to connect beyond where and as whom: the per-host choices a saved host resolves to
/// (CONN-17, CONN-18, KEY-01…03). Quick connect uses the defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Algorithms to offer; empty lists mean SSHerlock's defaults.
    pub algorithms: AlgorithmPreferences,
    /// Whether to compress.
    pub compression: Compression,
    /// The keychain key and saved password to try first.
    pub credentials: SavedCredentials,
}

/// The saved key and password a connection tries before the agent and prompts, still sealed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedCredentials {
    /// A keychain key.
    pub key: Option<Id<Key>>,
    /// A sealed password.
    pub password: Option<Sealed>,
}

/// [`SavedCredentials`] opened for logging in.
#[derive(Debug, Default)]
pub struct OpenedCredentials {
    /// The key, decrypted from the vault (but still under its own passphrase, if it has one).
    pub key: Option<StoredKey>,
    /// The password.
    pub password: Option<SecretString>,
}

impl SavedCredentials {
    /// Whether there's nothing saved to try.
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && self.password.is_none()
    }

    /// Opens the credentials with `vault`, finding the key among `keys`. A key that has been
    /// deleted since is left out. Quick: the vault's cipher is fast, and a key's own
    /// passphrase is only tried later, off the UI thread.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if a secret can't be decrypted.
    pub fn open(&self, keys: &[Key], vault: &Vault) -> Result<OpenedCredentials, StoreError> {
        let key = match self.key.and_then(|id| keys.iter().find(|k| k.id == id)) {
            Some(key) => Some(StoredKey {
                label: key.title().to_owned(),
                fingerprint: key.fingerprint.clone(),
                private_key: vault.open_string(&key.private_key)?,
                passphrase: match &key.protection {
                    KeyProtection::SavedPassphrase(sealed) => Some(vault.open_string(sealed)?),
                    KeyProtection::Unencrypted | KeyProtection::AskPassphrase => None,
                },
            }),
            None => None,
        };
        let password = self
            .password
            .as_ref()
            .map(|sealed| vault.open_string(sealed))
            .transpose()?;
        Ok(OpenedCredentials { key, password })
    }
}

/// Passphrases typed while logging in, remembered for a while for every tab (KEY-11).
#[derive(Clone, Debug, Default)]
pub struct Passphrases(pub PassphraseCache);

impl Global for Passphrases {}

/// Sets up the SSH feature: the passphrase cache every connection shares. Its actions are
/// handled by each pane and bound in `keymap/default.json`.
pub fn init(cx: &mut App) {
    cx.set_global(Passphrases::default());
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use ssherlock_core::keychain::KeyKind;
    use ssherlock_core::{ExposeSecret as _, Timestamp};
    use ssherlock_store::Store;
    use ssherlock_store::vault::MemoryKeyHolder;

    use super::*;

    #[test]
    fn saved_credentials_open_with_the_vault() {
        let store = Store::open_in_memory().unwrap();
        let vault = store
            .unlock_vault(&MemoryKeyHolder::default(), &MemoryKeyHolder::unavailable())
            .unwrap()
            .vault;
        let seal = |text: &str| vault.seal_str(&SecretString::from(text)).unwrap();
        let key = Key {
            id: Id::generate(),
            label: String::new(),
            kind: KeyKind::Ed25519,
            public_key: "ssh-ed25519 AAAA".into(),
            fingerprint: "SHA256:fp".into(),
            private_key: seal("-----BEGIN OPENSSH PRIVATE KEY-----"),
            protection: KeyProtection::SavedPassphrase(seal("open sesame")),
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        };
        let saved = SavedCredentials {
            key: Some(key.id),
            password: Some(seal("pw")),
        };
        let opened = saved.open(std::slice::from_ref(&key), &vault).unwrap();
        let stored = opened.key.unwrap();
        assert_eq!(stored.label, "SHA256:fp");
        assert_eq!(
            stored.private_key.expose_secret(),
            "-----BEGIN OPENSSH PRIVATE KEY-----"
        );
        assert_eq!(stored.passphrase.unwrap().expose_secret(), "open sesame");
        assert_eq!(opened.password.unwrap().expose_secret(), "pw");

        // A deleted key is left out; nothing saved opens to nothing.
        let opened = saved.open(&[], &vault).unwrap();
        assert!(opened.key.is_none() && opened.password.is_some());
        assert!(SavedCredentials::default().is_empty());
        // A secret sealed by another vault is an error, not a silent skip.
        let other = Store::open_in_memory()
            .unwrap()
            .unlock_vault(&MemoryKeyHolder::default(), &MemoryKeyHolder::unavailable())
            .unwrap()
            .vault;
        assert!(saved.open(&[key], &other).is_err());
    }
}
