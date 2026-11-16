//! A remote shell that survives network drops: connect, log in, open a shell, watch the
//! connection with keep-alives, and reconnect with backoff when it dies.
//!
//! **Service** layer. [`spawn_shell`] returns a [`PtyIo`] at once and does everything else in a
//! background task, reporting progress as [`SessionEvent::State`]. The terminal keeps the same
//! `PtyIo` across reconnects; each reconnect opens a fresh shell into it. The task ends, closing
//! the connection, when the shell exits, the connection can't be restored, or the `PtyIo` is
//! dropped (the tab closed).

use std::error::Error as _;
use std::time::Duration;

use ssherlock_terminal::{PtyExit, PtyIo, PtyOutput, PtyPeer, TerminalSize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::SshError;
use crate::auth::{AuthMethod, PassphraseCache};
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

/// Starts a remote shell for a terminal of `size` and returns the terminal's end of it.
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
) -> PtyIo {
    let (io, peer) = PtyIo::pair();
    let span = tracing::info_span!("remote_shell", host = %config.connect.host(), port = config.connect.port());
    let supervisor = Supervisor {
        config,
        host_keys,
        size,
        events,
        peer,
    };
    runtime.spawn(tracing::Instrument::instrument(supervisor.run(), span));
    io
}

struct Supervisor {
    config: RemoteShellConfig,
    host_keys: HostKeys,
    size: TerminalSize,
    events: mpsc::Sender<SessionEvent>,
    peer: PtyPeer,
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
                () = self.peer.output.closed() => break DisconnectReason::Closed,
            };
            match established {
                Ok((session, shell)) => {
                    if has_connected && !self.mark_reconnected().await {
                        break DisconnectReason::Closed;
                    }
                    has_connected = true;
                    attempt = 0;
                    match self.connected(&session, shell).await {
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
        tracing::info!(?reason, "remote shell finished");
        if reason != DisconnectReason::Closed {
            let exit = match &reason {
                DisconnectReason::ShellExited(exit) => exit.clone(),
                _ => PtyExit::Unknown,
            };
            // The terminal may already be gone; then there's nobody to tell.
            let _ = self.peer.output.send(PtyOutput::Exited(exit)).await;
        }
        self.state(ConnectionState::Disconnected(reason)).await;
    }

    /// Connects, logs in and opens a shell.
    async fn establish(&self) -> Result<(Session, OpenShell), SshError> {
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
        let shell = session.open_shell_channel(self.size).await?;
        Ok((session, shell))
    }

    /// Runs the shell until it ends, the terminal closes, or keep-alives say the connection died.
    async fn connected(&mut self, session: &Session, shell: OpenShell) -> Stretch {
        self.state(ConnectionState::Connected { latency: None })
            .await;
        let end = {
            let Self {
                peer,
                size,
                config,
                events,
                ..
            } = self;
            tokio::select! {
                end = bridge(shell, peer, size) => Some(end),
                () = heartbeat(session, config.keep_alive, events) => None,
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

    /// Waits `delay` before a reconnect, tracking resizes. Typing while disconnected is dropped:
    /// replaying it into a fresh shell could run half-typed commands. Returns `false` if the
    /// terminal closed meanwhile.
    async fn wait(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return true,
                command = self.peer.input.recv() => match command {
                    Some(ssherlock_terminal::PtyInput::Resize(size)) => self.size = size,
                    Some(ssherlock_terminal::PtyInput::Write(_)) => {}
                    None => return false,
                },
            }
        }
    }

    /// Writes a dim line into the terminal so the new shell's output doesn't run on from the
    /// old prompt. Returns `false` if the terminal is gone.
    async fn mark_reconnected(&self) -> bool {
        self.peer
            .output
            .send(PtyOutput::Data(RECONNECTED_NOTICE.to_vec()))
            .await
            .is_ok()
    }

    async fn state(&self, state: ConnectionState) {
        if self.events.send(SessionEvent::State(state)).await.is_err() {
            // The UI stopped listening; the PtyIo closing will end this task too.
            tracing::debug!("connection state not delivered: no listener");
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

/// An error and its causes as one line, for the user: `could not connect to h:22: refused`.
fn describe(error: &SshError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
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
