//! SSH connections on screen: the **view** layer of the SSH feature.
//!
//! [`RemoteTerminal`] shows a remote shell with its host key and login dialogs and a status bar;
//! [`QuickConnectBar`] turns typed `user@host:port` into a connection. The pane hosts both.
//! Connecting, logging in and reconnecting all happen in `ssherlock-ssh`.

pub mod actions;
pub mod dialogs;
pub mod prompts;
pub mod quick_connect;
pub mod remote_terminal;
pub mod status;

use std::path::PathBuf;

use gpui::{App, Global, SharedString};
pub use prompts::{SessionPrompts, SessionPromptsEvent};
pub use quick_connect::{QuickConnectBar, QuickConnectEvent};
pub use remote_terminal::RemoteTerminal;
use ssherlock_core::keychain::{Key, KeyProtection};
use ssherlock_core::{AlgorithmPreferences, Compression, ConnectTarget, Id, Sealed, SecretString};
use ssherlock_ssh::auth::{AuthMethod, default_key_files};
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::{ConnectConfig, DEFAULT_PORT};
use ssherlock_ssh::{HostKeys, PassphraseCache, RemoteShellConfig, StoredKey};
use ssherlock_store::{StoreError, Vault};

use crate::hosts::HostsModel;
use crate::notify;
use crate::runtime::TokioHandle;

/// Session events in flight to a view: a few state changes and at most one question at a time
/// (the session waits for each answer), so a small buffer never fills in practice.
pub const EVENT_CAPACITY: usize = 16;

/// What starting a connection to a target needs, worked out the same way for a terminal tab and
/// a forwarding-only connection.
#[derive(Debug)]
pub struct PreparedConnection {
    /// `user@host[:port]`, for status bars and titles.
    pub label: SharedString,
    /// What to start the connection with, or why it can't start.
    pub setup: Result<ConnectionSetup, &'static str>,
}

/// The parts of a connection that's ready to start.
#[derive(Debug)]
pub struct ConnectionSetup {
    /// Where, as whom, how to log in, keep-alives and reconnects.
    pub config: RemoteShellConfig,
    /// The `known_hosts` files to check against.
    pub host_keys: HostKeys,
    /// The Tokio runtime to run it on.
    pub runtime: tokio::runtime::Handle,
}

/// Prepares a connection to `target` with `options`. Missing parts come from the
/// [`SshEnvironment`]: the local user name, port 22, the default key files. Saved credentials
/// are opened from the vault here, so they're tried first.
pub fn prepare_connection(
    target: &ConnectTarget,
    options: &ConnectOptions,
    cx: &mut App,
) -> PreparedConnection {
    let env = cx.try_global::<SshEnvironment>().cloned();
    let tokio = cx.try_global::<TokioHandle>().map(|t| t.handle().clone());
    let user = target
        .user
        .clone()
        .or_else(|| env.as_ref().and_then(|env| env.local_user.clone()));
    let port = target.port.map_or(DEFAULT_PORT, ssherlock_core::Port::get);
    let label = match &user {
        Some(user) => format!("{user}@{}", host_label(target, port)),
        None => host_label(target, port),
    };
    let setup = match (env, tokio, user) {
        (Some(env), Some(runtime), Some(user)) => {
            let connect = ConnectConfig::new(target.address.to_string(), user)
                .with_port(port)
                .with_algorithms(options.algorithms.clone())
                .with_compression(options.compression);
            let key_files = env
                .ssh_dir
                .as_deref()
                .map(default_key_files)
                .unwrap_or_default();
            let saved = open_saved(options, cx);
            let mut config = RemoteShellConfig::new(
                connect,
                AuthMethod::with_saved(saved.key, saved.password, key_files),
            );
            config.passphrases = cx.try_global::<Passphrases>().map(|p| p.0.clone());
            let host_keys = HostKeys::new(
                env.known_hosts,
                env.read_only_known_hosts,
                HostHashing::Hashed,
            );
            Ok(ConnectionSetup {
                config,
                host_keys,
                runtime,
            })
        }
        (None, _, _) => Err("SSH is not set up: there is no data directory"),
        (_, None, _) => Err("the app's async runtime isn't running"),
        (_, _, None) => Err("no user name: connect as user@host"),
    };
    PreparedConnection {
        label: label.into(),
        setup,
    }
}

/// Opens the saved key and password `options` name, so the login can try them first. If they
/// can't be opened, says why and logs in without them (the agent, key files and prompts still
/// work).
fn open_saved(options: &ConnectOptions, cx: &mut App) -> OpenedCredentials {
    if options.credentials.is_empty() {
        return OpenedCredentials::default();
    }
    let Some(model) = HostsModel::global(cx) else {
        return OpenedCredentials::default();
    };
    let opened = {
        let model = model.read(cx);
        match model.vault() {
            Some(vault) => options
                .credentials
                .open(model.keys(), vault)
                .map_err(|error| error.to_string()),
            None => Err(model
                .vault_problem()
                .map_or_else(String::new, |p| p.to_string())),
        }
    };
    opened.unwrap_or_else(|reason| {
        tracing::warn!(%reason, "logging in without the saved credentials");
        notify::info(format!("Saved credentials weren't used: {reason}"), cx);
        OpenedCredentials::default()
    })
}

/// `host` or `host:port` (IPv6 bracketed) for labels, leaving out the default port.
fn host_label(target: &ConnectTarget, port: u16) -> String {
    if port == DEFAULT_PORT {
        target.address.to_string()
    } else {
        format!("{}:{port}", target.address.to_bracketed())
    }
}

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
