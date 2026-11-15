//! SSH sessions: connection, authentication, channels, port forwarding and SFTP.
//!
//! This is a **service** crate. It talks to the UI only through events sent over channels.
#![forbid(unsafe_code)]

mod error;
pub mod host_keys;
pub mod known_hosts;
pub mod session;

pub use error::SshError;
pub use host_keys::HostKeys;
pub use session::{ConnectConfig, Session, SessionEvent};
