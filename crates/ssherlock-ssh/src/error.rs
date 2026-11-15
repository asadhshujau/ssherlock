//! The crate's error type.

use std::path::PathBuf;

/// Everything that can go wrong in `ssherlock-ssh`.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// A `known_hosts` file exists but couldn't be read.
    #[error("could not read known_hosts at {path}")]
    KnownHostsRead {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },

    /// A `known_hosts` file couldn't be written.
    #[error("could not write known_hosts at {path}")]
    KnownHostsWrite {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },

    /// A public key couldn't be encoded in OpenSSH format.
    #[error("could not encode the {algorithm} public key")]
    KeyEncoding {
        /// The key's algorithm, e.g. `ssh-ed25519`.
        algorithm: String,
        /// Why.
        #[source]
        source: russh::keys::ssh_key::Error,
    },

    /// The operating system's random number generator failed.
    #[error("could not get random bytes from the operating system")]
    Random(#[from] rand::rngs::SysError),
}
