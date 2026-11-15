//! SSH sessions: connection, authentication, channels, port forwarding and SFTP.
//!
//! This is a **service** crate. It talks to the UI only through events sent over channels.
#![forbid(unsafe_code)]

pub mod auth;
pub mod connection;
mod error;
pub mod host_keys;
pub mod known_hosts;
pub mod session;
mod shell;

pub use auth::{AgentSocket, AuthMethod};
pub use connection::{ConnectionState, RemoteShellConfig, spawn_shell};
pub use error::SshError;
pub use host_keys::HostKeys;
/// The SSH public key type used throughout this crate's API.
pub use russh::keys::PublicKey;
pub use session::{ConnectConfig, Session, SessionEvent};
