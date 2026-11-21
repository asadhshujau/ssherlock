//! SSH sessions: connection, authentication, channels, port forwarding and SFTP.
//!
//! This is a **service** crate. It talks to the UI only through events sent over channels.
#![forbid(unsafe_code)]

pub mod algorithms;
pub mod auth;
pub mod authorized_keys;
pub mod connection;
mod error;
pub mod forwarding;
pub mod host_keys;
pub mod keys;
pub mod known_hosts;
pub mod session;
mod shell;

pub use algorithms::{AlgorithmPreferences, Compression};
pub use auth::{AgentSocket, AuthMethod, PassphraseCache, StoredKey};
pub use authorized_keys::KeyInstall;
pub use connection::{
    ConnectionHandle, ConnectionState, RemoteShellConfig, spawn_forwarding, spawn_shell,
};
pub use error::SshError;
pub use forwarding::{ForwardStats, ForwardStatus, RunningForward};
pub use host_keys::HostKeys;
/// The SSH public key type used throughout this crate's API.
pub use russh::keys::PublicKey;
pub use session::{ConnectConfig, Session, SessionEvent};
