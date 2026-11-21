//! A connection that survives network drops: connect, log in, open a shell (or not), watch the
//! connection with keep-alives, and reconnect with backoff when it dies.
//!
//! **Service** layer. [`spawn_shell`] returns a [`PtyIo`] at once and does everything else in a
//! background task, reporting progress as [`SessionEvent::State`]. The terminal keeps the same
//! `PtyIo` across reconnects; each reconnect opens a fresh shell into it. The task ends, closing
//! the connection, when the shell exits, the connection can't be restored, or the `PtyIo` is
//! dropped (the tab closed). [`spawn_forwarding`] is the same without a shell, for a
//! forwarding-only connection (FWD-06), and ends when its handle is dropped.
//!
//! Both return a [`ConnectionHandle`] that asks the task for work on the same connection:
//! adding a key to `authorized_keys`, or starting port forwards, which follow the connection
//! through reconnects.

use std::sync::Arc;
use std::time::Duration;

use ssherlock_core::forwarding::Forward;
use ssherlock_terminal::{PtyExit, PtyIo, PtyOutput, PtyPeer, TerminalSize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::SshError;
use crate::auth::{AuthMethod, PassphraseCache};
use crate::authorized_keys::KeyInstall;
use crate::error::describe;
use crate::forwarding::{self, RunningForward, Sessions};
use crate::host_keys::HostKeys;
use crate::session::{ConnectConfig, Session, SessionEvent};
use crate::shell::{OpenShell, ShellEnd, bridge};

/// Written into the terminal between the old shell's output and the new one's: a new line,
/// then "reconnected" in faint text (SGR 2, reset by SGR 22).
const RECONNECTED_NOTICE: &[u8] = b"\r\n\x1b[2m-- reconnected --\x1b[22m\r\n";

/// How often to check that the server still answers, and when to give up on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    /// Time between pings. Each ping must be answered within this time too.
    pub interval: Duration,
    /// Unanswered pings in a row after which the connection counts as lost.
    pub max_missed: u32,
}

impl Default for KeepAlive {
    /// A ping every 15 s; lost after 3 missed (about 45 s), like `ServerAliveInterval 15`.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            max_missed: 3,
        }
    }
}

/// How to retry after a connection that was working is lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Attempts before giving up; 0 never reconnects.
    pub max_attempts: u32,
    /// Wait before the first attempt; doubled for each one after.
    pub initial_delay: Duration,
    /// The longest wait between attempts.
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    /// 10 attempts, waiting 1 s, 2 s, 4 s … up to 30 s: about four minutes in all, long enough
    /// to ride out Wi-Fi switching or a laptop waking up.
    fn default() -> Self {
        Self {
            max_attempts: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    /// The wait before attempt `attempt` (1-based): exponential backoff, capped.
    pub fn delay(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(31);
        self.initial_delay
            .saturating_mul(1 << doublings)
            .min(self.max_delay)
    }
}

/// Everything [`spawn_shell`] needs to (re)establish a shell.
#[derive(Clone, Debug)]
pub struct RemoteShellConfig {
    /// Where to connect.
    pub connect: ConnectConfig,
    /// How to log in, tried in order on every (re)connect.
    pub methods: Vec<AuthMethod>,
    /// Liveness checks.
    pub keep_alive: KeepAlive,
    /// Retry policy after a drop.
    pub reconnect: ReconnectPolicy,
    /// Where typed passphrases are remembered between logins, if anywhere (KEY-11).
    pub passphrases: Option<PassphraseCache>,
}

impl RemoteShellConfig {
    /// `connect` and `methods` with the default keep-alive and reconnect policy.
    pub fn new(connect: ConnectConfig, methods: Vec<AuthMethod>) -> Self {
        Self {
            connect,
            methods,
            keep_alive: KeepAlive::default(),
            reconnect: ReconnectPolicy::default(),
            passphrases: None,
        }
    }
}

/// Where a connection is, for the status indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// Opening TCP and checking the host key. `attempt` is 0 for the first connection, then
    /// the reconnect attempt's number.
    Connecting {
        /// 0 first, then 1, 2, … for reconnects.
        attempt: u32,
    },
    /// Logging in.
    Authenticating,
    /// The shell is running. `latency` is the last keep-alive round trip, once measured.
    Connected {
        /// Last ping round trip.
        latency: Option<Duration>,
    },
    /// The connection was lost; the next attempt starts after `retry_in`.
    Reconnecting {
        /// 1-based attempt number.
        attempt: u32,
        /// Wait before it.
        retry_in: Duration,
    },
    /// Finished; nothing more will happen.
    Disconnected(DisconnectReason),
}

/// Why a connection finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The remote shell exited.
    ShellExited(PtyExit),
    /// The terminal was closed.
    Closed,
    /// Connecting or logging in failed in a way retrying won't fix (or on the first try).
    Failed(String),
    /// The connection was lost and every reconnect attempt failed.
    GaveUp {
        /// Attempts made.
        attempts: u32,
    },
}

/// Requests waiting for a connection to handle: one at a time from a user's action, so a small
/// buffer is plenty; a full one makes the sender wait.
const REQUEST_CAPACITY: usize = 4;

/// Asks a running connection ([`spawn_shell`] or [`spawn_forwarding`]) for work on the server:
/// adding a key to `authorized_keys`, or port forwards. Key requests wait while it's connecting
/// or reconnecting and run once it's connected; they fail with [`SshError::NotConnected`] if
/// the connection ends first.
///
/// For a forwarding-only connection the handle is what keeps it open: once every clone is
/// dropped, it disconnects.
#[derive(Clone, Debug)]
pub struct ConnectionHandle {
    requests: mpsc::Sender<Request>,
    sessions: Sessions,
    runtime: tokio::runtime::Handle,
    /// Set for a forwarding-only connection; dropping the last clone ends it.
    _keep_open: Option<Arc<DropGuard>>,
}

#[derive(Debug)]
enum Request {
    InstallKey {
        public_line: String,
        reply: oneshot::Sender<Result<KeyInstall, SshError>>,
    },
}

impl ConnectionHandle {
    /// Adds `public_line` to the logged-in user's `authorized_keys` over this connection (see
    /// [`Session::install_public_key`]).
    ///
    /// # Errors
    ///
    /// [`SshError::NotConnected`] if the connection ended before it could be done, or what
    /// [`Session::install_public_key`] reports.
    pub async fn install_public_key(&self, public_line: &str) -> Result<KeyInstall, SshError> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::InstallKey {
                public_line: public_line.to_owned(),
                reply,
            })
            .await
            .map_err(|_| SshError::NotConnected)?;
        answer.await.map_err(|_| SshError::NotConnected)?
    }

    /// Starts `forward` over this connection (FWD-01…03). It starts at once, waits out
    /// reconnects, and is restarted on each new session (FWD-07). It stops when the returned
    /// [`RunningForward`] is dropped or the connection finishes. A forward doesn't keep a
    /// forwarding-only connection open; this handle does.
    pub fn start_forward(&self, forward: Forward) -> RunningForward {
        forwarding::start(forward, self.sessions.clone(), &self.runtime)
    }
}

/// Starts a remote shell for a terminal of `size` and returns the terminal's end of it, with a
/// handle for asking the same connection to do more.
///
/// Progress and questions (host key, passwords) arrive on `events`. The first connection isn't
/// retried: if it fails, the user is better served by the error than by a wait. Once a shell has
/// run, a lost connection is retried per `config.reconnect`.
pub fn spawn_shell(
    config: RemoteShellConfig,
    host_keys: HostKeys,
    size: TerminalSize,
    events: mpsc::Sender<SessionEvent>,
    runtime: &tokio::runtime::Handle,
) -> (PtyIo, ConnectionHandle) {
    let (io, peer) = PtyIo::pair();
    let span = tracing::info_span!("remote_shell", host = %config.connect.host(), port = config.connect.port());
    let front = Front::Shell { peer, size };
    let handle = spawn(config, host_keys, front, None, events, runtime, span);
    (io, handle)
}

/// Starts a connection without a shell, for port forwarding only (FWD-06), and returns the
/// handle that starts forwards over it. It connects, logs in and reconnects exactly as
/// [`spawn_shell`] does, and disconnects once every clone of the handle is dropped.
pub fn spawn_forwarding(
    config: RemoteShellConfig,
    host_keys: HostKeys,
    events: mpsc::Sender<SessionEvent>,
    runtime: &tokio::runtime::Handle,
) -> ConnectionHandle {
    let closed = CancellationToken::new();
    let span = tracing::info_span!("forwarding", host = %config.connect.host(), port = config.connect.port());
    let front = Front::Headless {
        closed: closed.clone(),
    };
    let keep_open = Some(Arc::new(closed.drop_guard()));
    spawn(config, host_keys, front, keep_open, events, runtime, span)
}

fn spawn(
    config: RemoteShellConfig,
    host_keys: HostKeys,
    front: Front,
    keep_open: Option<Arc<DropGuard>>,
    events: mpsc::Sender<SessionEvent>,
    runtime: &tokio::runtime::Handle,
    span: tracing::Span,
) -> ConnectionHandle {
    let (requests, receiver) = mpsc::channel(REQUEST_CAPACITY);
    let (sessions_tx, sessions) = watch::channel(None);
    let supervisor = Supervisor {
        config,
        host_keys,
        front,
        events,
        requests: receiver,
        sessions: sessions_tx,
    };
    runtime.spawn(tracing::Instrument::instrument(supervisor.run(), span));
    ConnectionHandle {
        requests,
        sessions,
        runtime: runtime.clone(),
        _keep_open: keep_open,
    }
}

/// What a connection serves: a terminal's shell, or nothing but forwards and requests.
enum Front {
    /// A shell in a terminal; the connection ends when the terminal closes.
    Shell {
        peer: PtyPeer,
        /// Follows the terminal's resizes, so a reconnect opens the next shell at the right
        /// size.
        size: TerminalSize,
    },
    /// No shell; the connection ends when `closed` is cancelled (its last handle dropped).
    Headless { closed: CancellationToken },
}

impl Front {
    /// Resolves once whoever the connection serves has gone away.
    async fn closed(&self) {
        match self {
            Self::Shell { peer, .. } => peer.output.closed().await,
            Self::Headless { closed } => closed.cancelled().await,
        }
    }
}

struct Supervisor {
    config: RemoteShellConfig,
    host_keys: HostKeys,
    front: Front,
    events: mpsc::Sender<SessionEvent>,
    requests: mpsc::Receiver<Request>,
    /// The session forwards use: set while connected, `None` otherwise. Dropped when the
    /// supervisor ends, which ends the forwards too.
    sessions: watch::Sender<Option<Arc<Session>>>,
}

/// What ended one connected stretch.
enum Stretch {
    Finished(DisconnectReason),
    Lost,
}

impl Supervisor {
    async fn run(mut self) {
        let mut attempt = 0;
        let mut has_connected = false;
        let reason = loop {
            self.state(ConnectionState::Connecting { attempt }).await;
            let established = tokio::select! {
                result = self.establish() => result,
                () = self.front.closed() => break DisconnectReason::Closed,
            };
            match established {
                Ok((session, shell)) => {
                    if has_connected && !self.mark_reconnected().await {
                        break DisconnectReason::Closed;
                    }
                    has_connected = true;
                    attempt = 0;
                    let session = Arc::new(session);
                    self.sessions.send_replace(Some(Arc::clone(&session)));
                    let stretch = self.connected(&session, shell).await;
                    self.sessions.send_replace(None);
                    match stretch {
                        Stretch::Finished(reason) => {
                            disconnect(&session).await;
                            break reason;
                        }
                        Stretch::Lost => tracing::warn!("connection lost"),
                    }
                }
                Err(e) if has_connected && e.is_transient() => {
                    tracing::warn!(error = %describe(&e), "reconnect attempt failed");
                }
                Err(e) => break DisconnectReason::Failed(describe(&e)),
            }
            attempt += 1;
            if attempt > self.config.reconnect.max_attempts {
                break DisconnectReason::GaveUp {
                    attempts: attempt - 1,
                };
            }
            let retry_in = self.config.reconnect.delay(attempt);
            self.state(ConnectionState::Reconnecting { attempt, retry_in })
                .await;
            if !self.wait(retry_in).await {
                break DisconnectReason::Closed;
            }
        };
        tracing::info!(?reason, "connection finished");
        if let Front::Shell { peer, .. } = &self.front
            && reason != DisconnectReason::Closed
        {
            let exit = match &reason {
                DisconnectReason::ShellExited(exit) => exit.clone(),
                _ => PtyExit::Unknown,
            };
            // The terminal may already be gone; then there's nobody to tell.
            let _ = peer.output.send(PtyOutput::Exited(exit)).await;
        }
        self.state(ConnectionState::Disconnected(reason)).await;
    }

    /// Connects, logs in and, for a terminal, opens a shell.
    async fn establish(&self) -> Result<(Session, Option<OpenShell>), SshError> {
        let mut session = Session::connect(
            self.config.connect.clone(),
            self.host_keys.clone(),
            self.events.clone(),
        )
        .await?;
        self.state(ConnectionState::Authenticating).await;
        if let Some(cache) = &self.config.passphrases {
            session.use_passphrase_cache(cache.clone());
        }
        session.authenticate(&self.config.methods).await?;
        let shell = match &self.front {
            Front::Shell { size, .. } => Some(session.open_shell_channel(*size).await?),
            Front::Headless { .. } => None,
        };
        Ok((session, shell))
    }

    /// Runs until the shell ends (or, without one, the last handle goes), the terminal closes,
    /// or keep-alives say the connection died. Requests are handled meanwhile.
    async fn connected(&mut self, session: &Session, shell: Option<OpenShell>) -> Stretch {
        self.state(ConnectionState::Connected { latency: None })
            .await;
        let Self {
            front,
            config,
            events,
            requests,
            ..
        } = self;
        let end = {
            let served = async {
                match (front, shell) {
                    (Front::Shell { peer, size }, Some(shell)) => {
                        Some(bridge(shell, peer, size).await)
                    }
                    (Front::Shell { .. }, None) => {
                        // `establish` always opens a shell for a terminal.
                        tracing::warn!("connected without a shell for a terminal");
                        Some(ShellEnd::TerminalClosed)
                    }
                    (Front::Headless { closed }, _) => {
                        closed.cancelled().await;
                        Some(ShellEnd::TerminalClosed)
                    }
                }
            };
            let heartbeat = heartbeat(session, config.keep_alive, events);
            tokio::pin!(served, heartbeat);
            loop {
                tokio::select! {
                    end = &mut served => break end,
                    () = &mut heartbeat => break None,
                    // Handled in turn: the shell's output waits the moment a request takes.
                    Some(request) = requests.recv() => handle(session, request).await,
                }
            }
        };
        match end {
            Some(ShellEnd::TerminalClosed) => Stretch::Finished(DisconnectReason::Closed),
            Some(ShellEnd::ChannelClosed { exit: Some(exit) }) => {
                Stretch::Finished(DisconnectReason::ShellExited(exit))
            }
            // Closed without an exit status: the connection dropped, unless it's still up, in
            // which case the server simply ended the shell without saying how.
            Some(ShellEnd::ChannelClosed { exit: None }) if !session.is_closed() => {
                Stretch::Finished(DisconnectReason::ShellExited(PtyExit::Unknown))
            }
            Some(ShellEnd::ChannelClosed { exit: None }) | None => Stretch::Lost,
        }
    }

    /// Waits `delay` before a reconnect. A terminal's resizes are tracked; its typing is
    /// dropped, since replaying it into a fresh shell could run half-typed commands. Returns
    /// `false` if whoever the connection serves went away meanwhile.
    async fn wait(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        match &mut self.front {
            Front::Shell { peer, size } => loop {
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => return true,
                    command = peer.input.recv() => match command {
                        Some(ssherlock_terminal::PtyInput::Resize(new_size)) => *size = new_size,
                        Some(ssherlock_terminal::PtyInput::Write(_)) => {}
                        None => return false,
                    },
                }
            },
            Front::Headless { closed } => tokio::select! {
                () = tokio::time::sleep_until(deadline) => true,
                () = closed.cancelled() => false,
            },
        }
    }

    /// Writes a dim line into a terminal so the new shell's output doesn't run on from the old
    /// prompt. Returns `false` if the terminal is gone.
    async fn mark_reconnected(&self) -> bool {
        match &self.front {
            Front::Shell { peer, .. } => peer
                .output
                .send(PtyOutput::Data(RECONNECTED_NOTICE.to_vec()))
                .await
                .is_ok(),
            Front::Headless { .. } => true,
        }
    }

    async fn state(&self, state: ConnectionState) {
        if self.events.send(SessionEvent::State(state)).await.is_err() {
            // The UI stopped listening; the PtyIo closing will end this task too.
            tracing::debug!("connection state not delivered: no listener");
        }
    }
}

/// Does what `request` asks over `session` and sends back the outcome.
async fn handle(session: &Session, request: Request) {
    match request {
        Request::InstallKey { public_line, reply } => {
            let outcome = session.install_public_key(&public_line).await;
            if reply.send(outcome).is_err() {
                // The asker gave up (its tab closed); the key was still added.
                tracing::debug!("key install finished after its requester left");
            }
        }
    }
}

/// Pings the server every `keep_alive.interval`, reporting latency, and returns once
/// `keep_alive.max_missed` pings in a row go unanswered or the connection fails.
async fn heartbeat(session: &Session, keep_alive: KeepAlive, events: &mpsc::Sender<SessionEvent>) {
    let mut ticker = tokio::time::interval(keep_alive.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut missed = 0;
    loop {
        ticker.tick().await;
        match tokio::time::timeout(keep_alive.interval, session.ping()).await {
            Ok(Ok(latency)) => {
                missed = 0;
                // Latency updates are periodic; if the UI is behind, skipping one is harmless.
                let state = ConnectionState::Connected {
                    latency: Some(latency),
                };
                if events.try_send(SessionEvent::State(state)).is_err() {
                    tracing::trace!("latency update skipped: event queue full");
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %describe(&e), "keep-alive failed");
                return;
            }
            Err(_) => {
                missed += 1;
                tracing::debug!(missed, "keep-alive unanswered");
                if missed >= keep_alive.max_missed {
                    return;
                }
            }
        }
    }
}

async fn disconnect(session: &Session) {
    if let Err(e) = session.disconnect().await {
        // Already gone, which is what we wanted.
        tracing::debug!(error = %e, "connection was already closed");
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let policy = ReconnectPolicy {
            max_attempts: 50,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
        };
        for (attempt, expected_ms) in [
            (0, 500),
            (1, 500),
            (2, 1000),
            (3, 2000),
            (4, 4000),
            (5, 8000),
            (6, 10_000),
            (40, 10_000),
        ] {
            assert_eq!(
                policy.delay(attempt),
                Duration::from_millis(expected_ms),
                "attempt {attempt}"
            );
        }
    }

    #[test]
    fn default_policy_rides_out_a_few_minutes() {
        let policy = ReconnectPolicy::default();
        let total: Duration = (1..=policy.max_attempts).map(|a| policy.delay(a)).sum();
        assert!(
            total > Duration::from_secs(120) && total < Duration::from_secs(600),
            "{total:?}"
        );
    }

    #[test]
    fn errors_are_described_with_their_causes() {
        let error = SshError::Connect {
            host: "example.com".into(),
            port: 22,
            source: std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection refused",
            ),
        };
        assert_eq!(
            describe(&error),
            "could not connect to example.com:22: connection refused"
        );
    }
}
