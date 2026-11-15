//! The crate's error type.

use std::path::PathBuf;
use std::time::Duration;

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

    /// TCP to the server failed.
    #[error("could not connect to {host}:{port}")]
    Connect {
        /// The host as given.
        host: String,
        /// The port.
        port: u16,
        /// Why.
        #[source]
        source: std::io::Error,
    },

    /// The server didn't accept TCP and send its host key in time.
    #[error("timed out connecting to {host}:{port} after {} s", after.as_secs())]
    ConnectTimeout {
        /// The host as given.
        host: String,
        /// The port.
        port: u16,
        /// The timeout that ran out.
        after: Duration,
    },

    /// The user declined the server's host key (or wasn't there to be asked).
    #[error("the host key for {host} was not trusted")]
    HostKeyRejected {
        /// The host as named in `known_hosts`.
        host: String,
    },

    /// The server's host key is marked `@revoked`.
    #[error("the host key for {host} is revoked at {path} line {line}")]
    HostKeyRevoked {
        /// The host as named in `known_hosts`.
        host: String,
        /// The file that revokes it.
        path: PathBuf,
        /// The line that revokes it.
        line: usize,
    },

    /// The SSH protocol layer failed.
    #[error("SSH protocol error")]
    Protocol(#[from] russh::Error),

    /// Every login method was tried and refused, or none could be tried.
    #[error("could not log in as {user} (tried: {})", if tried.is_empty() { "nothing the server accepts".to_owned() } else { tried.join(", ") })]
    AuthFailed {
        /// Who tried to log in.
        user: String,
        /// The methods the server refused, in order.
        tried: Vec<&'static str>,
    },

    /// The user dismissed a login prompt.
    #[error("the login was cancelled")]
    AuthCancelled,

    /// A login prompt was answered with the wrong number of answers (a UI bug).
    #[error("a login prompt expected {expected} answers but got {got}")]
    PromptAnswerCount {
        /// Fields in the prompt.
        expected: usize,
        /// Answers received.
        got: usize,
    },

    /// The server kept sending keyboard-interactive prompts.
    #[error("the server sent too many login prompts")]
    TooManyPrompts,

    /// The server refused a channel request.
    #[error("the server refused to start a {request}")]
    ChannelRequestRefused {
        /// What was asked for: `pty` or `shell`.
        request: &'static str,
    },

    /// A channel closed before it was ready.
    #[error("the server closed the channel")]
    ChannelClosed,
}

impl SshError {
    /// Whether trying again later might work: network trouble, as opposed to a refused login, a
    /// rejected host key or a cancelled prompt, which would fail the same way again.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Connect { .. }
                | Self::ConnectTimeout { .. }
                | Self::Protocol(_)
                | Self::ChannelClosed
        )
    }
}
