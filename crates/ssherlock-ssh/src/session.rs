//! A live connection to an SSH server: TCP, key exchange and host key verification.
//!
//! **Service** layer. A [`Session`] never touches the UI: questions for the user (an unknown or
//! changed host key) go up as [`SessionEvent`]s carrying a `oneshot` reply, and the session waits
//! for the answer.

use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::{HashAlg, PublicKey, PublicKeyOrCertificate};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::Instant;
use tracing::Instrument as _;

use crate::SshError;
use crate::auth::{AuthMethod, AuthPrompt, Authenticator};
use crate::host_keys::HostKeys;
use crate::known_hosts::{HostKeyStatus, KnownHostName, KnownKey};

/// The standard SSH port.
pub const DEFAULT_PORT: u16 = 22;
/// How long [`Session::connect`] waits for TCP and the server's host key by default.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// What to connect to and how. Built with [`ConnectConfig::new`] and the `with_*` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectConfig {
    host: String,
    port: u16,
    username: String,
    connect_timeout: Duration,
}

impl ConnectConfig {
    /// Connect to `host` (a DNS name, or an IPv4/IPv6 literal without brackets) on port 22 as
    /// `username`.
    pub fn new(host: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            username: username.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Use `port` instead of 22.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Give up if TCP plus the key exchange up to the host key check take longer than this.
    /// Time spent waiting for the user to answer a host key question doesn't count.
    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The server's host name or address.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The server's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The user to log in as.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// See [`ConnectConfig::with_connect_timeout`].
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }
}

/// Something a session tells the UI, or asks it.
#[derive(Debug)]
pub enum SessionEvent {
    /// The server's host key isn't trusted yet; the user must decide.
    HostKey(HostKeyPrompt),
    /// Logging in needs something typed: a password, a passphrase, a 2FA code.
    Prompt(AuthPrompt),
    /// The server's pre-login banner (`/etc/issue.net` and the like), to show the user.
    Banner(String),
}

/// A question to the user about a host key that isn't trusted yet. Answer it with
/// [`HostKeyPrompt::answer`]; dropping it unanswered rejects the key.
#[derive(Debug)]
pub struct HostKeyPrompt {
    host: KnownHostName,
    key: PublicKey,
    concern: HostKeyConcern,
    reply: oneshot::Sender<HostKeyDecision>,
}

/// Why the user is being asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyConcern {
    /// First connection with this key type (trust on first use).
    New {
        /// Keys of other algorithms already known for the host, if any.
        other_algorithms: Vec<KnownKey>,
    },
    /// The host is known with a different key of the same algorithm. This is what a
    /// man-in-the-middle looks like, or a reinstalled server.
    Changed {
        /// The keys that were expected.
        known: Vec<KnownKey>,
    },
}

/// The user's answer to a [`HostKeyPrompt`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// Trust the key and remember it in `known_hosts`.
    Trust,
    /// Refuse it; the connection is closed.
    Reject,
}

impl HostKeyPrompt {
    /// The host as stored in `known_hosts` (`host` or `[host]:port`).
    pub fn host(&self) -> &KnownHostName {
        &self.host
    }

    /// The key the server presented.
    pub fn key(&self) -> &PublicKey {
        &self.key
    }

    /// Why the user is being asked.
    pub fn concern(&self) -> &HostKeyConcern {
        &self.concern
    }

    /// The key's `SHA256:…` fingerprint, as OpenSSH shows it.
    pub fn fingerprint(&self) -> String {
        self.key.fingerprint(HashAlg::Sha256).to_string()
    }

    /// The fingerprint's randomart picture, as `ssh-keygen -lv` draws it.
    pub fn randomart(&self) -> String {
        let header = format!("[{}]", self.key.algorithm().as_str());
        self.key.fingerprint(HashAlg::Sha256).to_randomart(&header)
    }

    /// Sends the user's decision to the waiting session.
    pub fn answer(self, decision: HostKeyDecision) {
        if self.reply.send(decision).is_err() {
            // The session gave up waiting (cancelled or timed out by the server); there is no
            // one left to tell, and nothing was trusted.
            tracing::warn!(
                host = self.host.as_str(),
                "host key answer arrived after the session ended"
            );
        }
    }
}

/// A connected SSH session.
pub struct Session {
    handle: client::Handle<ClientHandler>,
    config: ConnectConfig,
    events: mpsc::Sender<SessionEvent>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Opens TCP, runs the key exchange and verifies the server's host key against
    /// `host_keys`, asking the user through `events` when it isn't trusted yet.
    ///
    /// # Errors
    ///
    /// [`SshError::Connect`] / [`SshError::ConnectTimeout`] if the server can't be reached,
    /// [`SshError::HostKeyRejected`] / [`SshError::HostKeyRevoked`] if its key isn't accepted,
    /// [`SshError::Protocol`] for anything the SSH layer reports.
    pub async fn connect(
        config: ConnectConfig,
        host_keys: HostKeys,
        events: mpsc::Sender<SessionEvent>,
    ) -> Result<Self, SshError> {
        let span = tracing::info_span!("ssh", host = %config.host, port = config.port);
        async move {
            let handle = handshake(&config, host_keys, events.clone()).await?;
            tracing::info!("connected");
            Ok(Self {
                handle,
                config,
                events,
            })
        }
        .instrument(span)
        .await
    }

    /// Logs in as the configured user, trying `methods` in order until one works. Prompts go
    /// to the UI as [`SessionEvent::Prompt`].
    ///
    /// # Errors
    ///
    /// [`SshError::AuthFailed`] if every method was refused, [`SshError::AuthCancelled`] if the
    /// user dismissed a prompt, [`SshError::Protocol`] if the connection failed.
    pub async fn authenticate(&mut self, methods: &[AuthMethod]) -> Result<(), SshError> {
        let span =
            tracing::info_span!("auth", host = %self.config.host, user = %self.config.username);
        let mut authenticator = Authenticator {
            handle: &mut self.handle,
            user: &self.config.username,
            host: &self.config.host,
            events: &self.events,
        };
        authenticator.run(methods).instrument(span).await
    }

    /// What this session connected to.
    pub fn config(&self) -> &ConnectConfig {
        &self.config
    }

    /// Whether the connection has closed.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Closes the connection politely.
    ///
    /// # Errors
    ///
    /// [`SshError::Protocol`] if the connection is already gone.
    pub async fn disconnect(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await?;
        Ok(())
    }
}

async fn handshake(
    config: &ConnectConfig,
    host_keys: HostKeys,
    events: mpsc::Sender<SessionEvent>,
) -> Result<client::Handle<ClientHandler>, SshError> {
    let timed_out = || SshError::ConnectTimeout {
        host: config.host.clone(),
        port: config.port,
        after: config.connect_timeout,
    };
    let deadline = Instant::now() + config.connect_timeout;

    let stream = tokio::time::timeout_at(
        deadline,
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await
    .map_err(|_| timed_out())?
    .map_err(|source| SshError::Connect {
        host: config.host.clone(),
        port: config.port,
        source,
    })?;
    // Keystrokes should go out at once rather than wait for Nagle's algorithm.
    if let Err(e) = stream.set_nodelay(true) {
        tracing::warn!(error = %e, "could not disable Nagle's algorithm; typing may feel laggy");
    }

    // The deadline covers the key exchange only until the host key arrives: after that the
    // session may be waiting for the user, who can take as long as they like.
    let host_key_reached = Arc::new(Notify::new());
    let handler = ClientHandler {
        name: KnownHostName::new(&config.host, config.port),
        host_keys,
        events,
        host_key_reached: Arc::clone(&host_key_reached),
    };
    let russh_config = Arc::new(client::Config {
        nodelay: true,
        ..client::Config::default()
    });
    let handshake = client::connect_stream(russh_config, stream, handler);
    let watchdog = async {
        if tokio::time::timeout_at(deadline, host_key_reached.notified())
            .await
            .is_ok()
        {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        result = handshake => result,
        () = watchdog => Err(timed_out()),
    }
}

/// russh's callbacks for one connection.
pub(crate) struct ClientHandler {
    name: KnownHostName,
    host_keys: HostKeys,
    events: mpsc::Sender<SessionEvent>,
    host_key_reached: Arc<Notify>,
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, SshError> {
        self.host_key_reached.notify_one();
        let key = match server_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // Host certificates need `@cert-authority` support (CONN-05). Until then the
            // certificate's key is checked as a plain host key, which is OpenSSH's fallback too.
            PublicKeyOrCertificate::Certificate(cert) => {
                PublicKey::new(cert.public_key().clone(), "")
            }
        };
        verify_host_key(&self.name, key, &self.host_keys, &self.events).await?;
        Ok(true)
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), SshError> {
        if self
            .events
            .send(SessionEvent::Banner(banner.to_owned()))
            .await
            .is_err()
        {
            // Nobody is listening any more, so the connection is being torn down anyway.
            tracing::warn!("dropped the server banner: the session's listener is gone");
        }
        Ok(())
    }
}

/// Accepts a trusted key, refuses a revoked one, and asks the user about anything else.
async fn verify_host_key(
    name: &KnownHostName,
    key: PublicKey,
    host_keys: &HostKeys,
    events: &mpsc::Sender<SessionEvent>,
) -> Result<(), SshError> {
    let rejected = || SshError::HostKeyRejected {
        host: name.as_str().to_owned(),
    };
    let concern = match host_keys.check(name, &key).await? {
        HostKeyStatus::Trusted => return Ok(()),
        HostKeyStatus::Revoked { at } => {
            return Err(SshError::HostKeyRevoked {
                host: name.as_str().to_owned(),
                path: at.path,
                line: at.line,
            });
        }
        HostKeyStatus::Unknown { other_algorithms } => HostKeyConcern::New { other_algorithms },
        HostKeyStatus::Changed { known } => HostKeyConcern::Changed { known },
    };
    let changed = matches!(concern, HostKeyConcern::Changed { .. });
    let (reply, decision) = oneshot::channel();
    let prompt = HostKeyPrompt {
        host: name.clone(),
        key: key.clone(),
        concern,
        reply,
    };
    // With no one to ask, the key can't be trusted.
    events
        .send(SessionEvent::HostKey(prompt))
        .await
        .map_err(|_| rejected())?;
    match decision.await {
        Ok(HostKeyDecision::Trust) => {
            tracing::info!(changed, fingerprint = %key.fingerprint(HashAlg::Sha256), "user trusted host key");
            if let Err(e) = host_keys.trust(name, &key).await {
                // The user did trust it, so connect anyway; they'll just be asked again next time.
                tracing::warn!(error = %e, "could not save the trusted host key");
            }
            Ok(())
        }
        Ok(HostKeyDecision::Reject) | Err(_) => {
            tracing::info!(changed, "user rejected host key");
            Err(rejected())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::known_hosts::HostHashing;

    fn host_keys() -> HostKeys {
        let path =
            std::env::temp_dir().join(format!("ssherlock-session-unit-{}", std::process::id()));
        HostKeys::new(path, vec![], HostHashing::Plain)
    }

    #[tokio::test]
    async fn refused_connection_is_a_connect_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (tx, _rx) = mpsc::channel(1);
        let result = Session::connect(
            ConnectConfig::new("127.0.0.1", "u").with_port(port),
            host_keys(),
            tx,
        )
        .await;
        assert!(
            matches!(result, Err(SshError::Connect { port: p, .. }) if p == port),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        // Accepts TCP but never speaks SSH.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(socket);
        });
        let config = ConnectConfig::new("127.0.0.1", "u")
            .with_port(port)
            .with_connect_timeout(Duration::from_millis(300));
        let (tx, _rx) = mpsc::channel(1);
        let started = std::time::Instant::now();
        let result = Session::connect(config, host_keys(), tx).await;
        assert!(
            matches!(result, Err(SshError::ConnectTimeout { .. })),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        hold.abort();
    }

    #[test]
    fn config_builder() {
        let config = ConnectConfig::new("example.com", "alice");
        assert_eq!(
            (config.host(), config.port(), config.username()),
            ("example.com", 22, "alice")
        );
        assert_eq!(config.connect_timeout(), DEFAULT_CONNECT_TIMEOUT);
        let config = config
            .with_port(2222)
            .with_connect_timeout(Duration::from_secs(3));
        assert_eq!(
            (config.port(), config.connect_timeout()),
            (2222, Duration::from_secs(3))
        );
    }
}
