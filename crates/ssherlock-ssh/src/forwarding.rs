//! Port forwarding over a live connection: local (`-L`), remote (`-R`) and dynamic SOCKS
//! (`-D`) forwards, with status and byte counters (FWD-01…03, FWD-07).
//!
//! **Service** layer. A forward runs as a task on Tokio and gets the connection's current
//! [`Session`] through a `watch` channel, which holds `None` while the connection is being
//! made or remade. So a forward outlives reconnects: a local listener stays open throughout,
//! and a remote forward asks each new session to listen again (FWD-07, restart on reconnect).
//! It ends when its [`RunningForward`] is dropped, or when the connection finishes for good.
//! Each connection through it is its own task, ended with it.

mod listen;
mod pump;
mod socks;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{ChannelOpenHandle, Msg};
use russh::{Channel, ChannelOpenFailure};
use ssherlock_core::forwarding::{Bind, Destination, Forward, Listen};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::Instrument as _;

use crate::SshError;
use crate::error::describe;
use crate::session::Session;

/// The connection's current session, or `None` while it's connecting or reconnecting.
pub(crate) type Sessions = watch::Receiver<Option<Arc<Session>>>;

/// How long a SOCKS client gets to say where it wants to go.
const SOCKS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait before asking a reconnected server to listen again, and how many times,
/// when it refuses: the dead connection's old listener can hold the port until the server
/// notices the connection is gone.
const RELISTEN_DELAY: Duration = Duration::from_secs(5);
const RELISTEN_ATTEMPTS: u32 = 6;
/// Pause after a failed `accept`, which is usually the process running out of file
/// descriptors, so the loop doesn't spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(200);
/// Incoming connections a remote forward has yet to pick up. They arrive as fast as programs on
/// the server connect; when the queue is full, more are refused rather than block the session.
const INCOMING_CAPACITY: usize = 32;

/// Where a forward is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardStatus {
    /// Opening its port.
    Starting,
    /// Listening, and forwarding what connects.
    Active,
    /// Waiting for the SSH connection, which is being made or remade. A local port stays
    /// open; connections to it are closed until the session is back.
    Waiting,
    /// It couldn't run, for the reason given (in use, refused by the server). Final.
    Failed(String),
    /// Stopped, or the connection finished. Final.
    Stopped,
}

impl ForwardStatus {
    /// Whether nothing more will happen.
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::Stopped)
    }
}

/// Traffic through one forward since it started (FWD-07).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForwardStats {
    /// Bytes sent from this machine's side towards the server's.
    pub uploaded: u64,
    /// Bytes received from the server's side.
    pub downloaded: u64,
    /// Connections open now.
    pub active_connections: usize,
    /// Connections since the start.
    pub total_connections: u64,
    /// Why the last connection that failed did, if one has.
    pub last_error: Option<String>,
}

/// The live counts behind [`ForwardStats`], updated by each connection's task.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    uploaded: AtomicU64,
    downloaded: AtomicU64,
    active: AtomicUsize,
    total: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Counters {
    fn snapshot(&self) -> ForwardStats {
        ForwardStats {
            uploaded: self.uploaded.load(Ordering::Relaxed),
            downloaded: self.downloaded.load(Ordering::Relaxed),
            active_connections: self.active.load(Ordering::Relaxed),
            total_connections: self.total.load(Ordering::Relaxed),
            last_error: self.last_error.lock().ok().and_then(|e| e.clone()),
        }
    }

    fn failed(&self, error: &SshError) {
        let text = describe(error);
        tracing::warn!(error = %text, "forwarded connection failed");
        if let Ok(mut last) = self.last_error.lock() {
            *last = Some(text);
        }
    }
}

/// Counts one connection as active while it lives.
struct Tracked<'a>(&'a Counters);

impl<'a> Tracked<'a> {
    fn new(counters: &'a Counters) -> Self {
        counters.total.fetch_add(1, Ordering::Relaxed);
        counters.active.fetch_add(1, Ordering::Relaxed);
        Self(counters)
    }
}

impl Drop for Tracked<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A forward that is running. Dropping it stops the forward and every connection through it.
#[derive(Debug)]
pub struct RunningForward {
    forward: Forward,
    status: watch::Receiver<ForwardStatus>,
    counters: Arc<Counters>,
    _stop: DropGuard,
}

impl RunningForward {
    /// What it forwards.
    pub fn forward(&self) -> &Forward {
        &self.forward
    }

    /// Where it is now.
    pub fn status(&self) -> ForwardStatus {
        self.status.borrow().clone()
    }

    /// A receiver that wakes on every status change, for a view to follow.
    pub fn watch(&self) -> watch::Receiver<ForwardStatus> {
        self.status.clone()
    }

    /// Its traffic so far.
    pub fn stats(&self) -> ForwardStats {
        self.counters.snapshot()
    }
}

/// Starts `forward` on the connection whose sessions `sessions` follows.
pub(crate) fn start(
    forward: Forward,
    sessions: Sessions,
    runtime: &tokio::runtime::Handle,
) -> RunningForward {
    let (status_tx, status) = watch::channel(ForwardStatus::Starting);
    let counters = Arc::new(Counters::default());
    let stop = CancellationToken::new();
    let span = tracing::info_span!("forward", forward = %forward.to_openssh());
    let task = Task {
        sessions,
        status: status_tx,
        counters: Arc::clone(&counters),
        stop: stop.clone(),
    };
    runtime.spawn(task.run(forward.clone()).instrument(span));
    RunningForward {
        forward,
        status,
        counters,
        _stop: stop.drop_guard(),
    }
}

/// The task behind one [`RunningForward`].
struct Task {
    sessions: Sessions,
    status: watch::Sender<ForwardStatus>,
    counters: Arc<Counters>,
    stop: CancellationToken,
}

impl Task {
    async fn run(self, forward: Forward) {
        let outcome = match forward {
            Forward::Local {
                listen,
                destination,
            } => self.listen_here(listen, Some(destination)).await,
            Forward::Dynamic { listen } => self.listen_here(listen, None).await,
            Forward::Remote {
                listen,
                destination,
            } => self.listen_on_server(listen, destination).await,
        };
        let end = match outcome {
            Ok(()) => ForwardStatus::Stopped,
            Err(error) => {
                let text = describe(&error);
                tracing::warn!(error = %text, "forward failed");
                ForwardStatus::Failed(text)
            }
        };
        tracing::info!(status = ?end, "forward finished");
        self.status.send_replace(end);
        // Ends the connections still going through it.
        self.stop.cancel();
    }

    fn set(&self, status: ForwardStatus) {
        self.status.send_if_modified(|current| {
            let changed = *current != status;
            *current = status;
            changed
        });
    }

    /// Runs a local or (without `destination`) dynamic forward: listens here and carries each
    /// connection through the current session.
    async fn listen_here(
        &self,
        listen: Listen,
        destination: Option<Destination>,
    ) -> Result<(), SshError> {
        let listeners = listen::Listeners::bind(listen).await?;
        tracing::info!("listening");
        let mut sessions = self.sessions.clone();
        loop {
            let session = sessions.borrow_and_update().clone();
            self.set(if session.is_some() {
                ForwardStatus::Active
            } else {
                ForwardStatus::Waiting
            });
            tokio::select! {
                () = self.stop.cancelled() => return Ok(()),
                changed = sessions.changed() => {
                    if changed.is_err() {
                        // The connection finished for good.
                        return Ok(());
                    }
                }
                accepted = listeners.accept() => match (accepted, session) {
                    (Ok((socket, peer)), Some(session)) => {
                        self.carry_out(socket, peer, session, destination.clone());
                    }
                    (Ok((_, peer)), None) => {
                        // Closing it at once tells the program to try again later.
                        tracing::debug!(%peer, "closed a connection while the SSH connection is down");
                    }
                    (Err(error), _) => {
                        tracing::warn!(%error, "could not accept a connection");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                },
            }
        }
    }

    /// Carries one local connection to `destination`, or for a SOCKS proxy wherever it asks.
    fn carry_out(
        &self,
        mut socket: TcpStream,
        peer: SocketAddr,
        session: Arc<Session>,
        destination: Option<Destination>,
    ) {
        let counters = Arc::clone(&self.counters);
        let stop = self.stop.child_token();
        let work = async move {
            let _tracked = Tracked::new(&counters);
            let outcome = async {
                let (host, port) = if let Some(d) = &destination {
                    (d.host.to_string(), d.port.get())
                } else {
                    let target = tokio::time::timeout(
                        SOCKS_HANDSHAKE_TIMEOUT,
                        socks::handshake(&mut socket),
                    )
                    .await
                    .map_err(|_| SshError::SocksRequest {
                        reason: "the client did not finish the handshake in time",
                    })??;
                    (target.host, target.port)
                };
                let opened = session.open_direct(&host, port, peer).await;
                // The session is only needed to open the channel; holding it longer would keep
                // a dead connection's state alive.
                drop(session);
                let channel = match (opened, destination.is_none()) {
                    (Ok(channel), false) => channel,
                    (Ok(channel), true) => {
                        socks::reply(&mut socket, socks::Reply::Succeeded).await?;
                        channel
                    }
                    (Err(error), is_socks) => {
                        if is_socks {
                            let _ = socks::reply(&mut socket, socks::Reply::GeneralFailure).await;
                        }
                        return Err(error);
                    }
                };
                pump::pump(socket, channel.into_stream(), &counters).await;
                Ok(())
            }
            .await;
            if let Err(error) = outcome {
                counters.failed(&error);
            }
        };
        tokio::spawn(
            async move {
                tokio::select! {
                    () = stop.cancelled() => {}
                    () = work => {}
                }
            }
            .in_current_span(),
        );
    }

    /// Runs a remote forward: asks each session in turn to listen, and carries what connects
    /// there to `destination` from this machine.
    async fn listen_on_server(
        &self,
        listen: Listen,
        destination: Destination,
    ) -> Result<(), SshError> {
        let mut sessions = self.sessions.clone();
        let mut has_listened = false;
        loop {
            self.set(ForwardStatus::Waiting);
            // The next session that is up.
            let session = loop {
                if let Some(session) = sessions.borrow_and_update().clone() {
                    break session;
                }
                tokio::select! {
                    () = self.stop.cancelled() => return Ok(()),
                    changed = sessions.changed() => {
                        if changed.is_err() {
                            return Ok(());
                        }
                    }
                }
            };
            let mut incoming = match self.ask_to_listen(&session, listen, has_listened).await {
                Ok(Some(incoming)) => incoming,
                // Stopped while asking.
                Ok(None) => return Ok(()),
                Err(error) => return Err(error),
            };
            has_listened = true;
            tracing::info!("the server is listening");
            self.set(ForwardStatus::Active);
            loop {
                tokio::select! {
                    () = self.stop.cancelled() => {
                        session.stop_listening(listen).await;
                        return Ok(());
                    }
                    changed = sessions.changed() => {
                        if changed.is_err() {
                            return Ok(());
                        }
                        let is_same = sessions
                            .borrow()
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &session));
                        if !is_same {
                            // Lost; wait for the next session and listen again there.
                            break;
                        }
                    }
                    connection = incoming.recv() => match connection {
                        Some(connection) => self.carry_back(connection, destination.clone()),
                        // The session ended under us.
                        None => break,
                    },
                }
            }
        }
    }

    /// Asks `session` to listen on `listen`. After a reconnect a refusal is retried a few
    /// times, in case the old connection still holds the port; on the first try it's final.
    /// `None` if the forward was stopped meanwhile.
    async fn ask_to_listen(
        &self,
        session: &Session,
        listen: Listen,
        is_relisten: bool,
    ) -> Result<Option<mpsc::Receiver<Incoming>>, SshError> {
        let mut attempt = 0;
        loop {
            match session.listen_remote(listen).await {
                Ok(incoming) => return Ok(Some(incoming)),
                Err(SshError::RemoteListenRefused { .. })
                    if is_relisten && attempt < RELISTEN_ATTEMPTS && !session.is_closed() =>
                {
                    attempt += 1;
                    tracing::info!(attempt, "the server refused to listen again yet; retrying");
                    tokio::select! {
                        () = self.stop.cancelled() => return Ok(None),
                        () = tokio::time::sleep(RELISTEN_DELAY) => {}
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Connects one connection the server forwarded to `destination` here and carries it.
    fn carry_back(&self, connection: Incoming, destination: Destination) {
        let counters = Arc::clone(&self.counters);
        let stop = self.stop.child_token();
        let work = async move {
            let _tracked = Tracked::new(&counters);
            let address = destination.to_string();
            let socket =
                match TcpStream::connect((destination.host.to_string(), destination.port.get()))
                    .await
                {
                    Ok(socket) => socket,
                    Err(source) => {
                        connection
                            .reply
                            .reject(ChannelOpenFailure::ConnectFailed)
                            .await;
                        counters.failed(&SshError::ForwardConnect {
                            destination: address,
                            source,
                        });
                        return;
                    }
                };
            connection.reply.accept().await;
            tracing::debug!(from = %connection.originator, "carrying a connection from the server");
            pump::pump(socket, connection.channel.into_stream(), &counters).await;
        };
        tokio::spawn(
            async move {
                tokio::select! {
                    () = stop.cancelled() => {}
                    () = work => {}
                }
            }
            .in_current_span(),
        );
    }
}

/// A connection the server accepted on a remote forward's port, waiting to be accepted or
/// refused here.
pub(crate) struct Incoming {
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    /// The address of the program that connected on the server, for the log.
    originator: String,
}

/// Remote forwards' incoming connections by the server port they arrived on. Shared by a
/// session and its russh handler, which looks up where each incoming channel goes.
#[derive(Debug, Default)]
pub(crate) struct Routes(Mutex<HashMap<u32, mpsc::Sender<Incoming>>>);

impl Routes {
    /// Hands a connection that arrived on `port` to its forward. One that nobody is listening
    /// for, or that the forward has no room for, is refused.
    pub(crate) fn route(
        &self,
        port: u32,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        originator: String,
    ) {
        let sender = self
            .0
            .lock()
            .ok()
            .and_then(|routes| routes.get(&port).cloned());
        let Some(sender) = sender else {
            // Dropping the reply refuses the channel.
            tracing::warn!(
                port,
                "refused a forwarded connection for a port with no forward"
            );
            return;
        };
        let connection = Incoming {
            channel,
            reply,
            originator,
        };
        if let Err(error) = sender.try_send(connection) {
            tracing::warn!(port, %error, "refused a forwarded connection");
        }
    }

    fn add(&self, port: u32, sender: mpsc::Sender<Incoming>) {
        if let Ok(mut routes) = self.0.lock() {
            routes.insert(port, sender);
        }
    }

    fn remove(&self, port: u32) {
        if let Ok(mut routes) = self.0.lock() {
            routes.remove(&port);
        }
    }
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("originator", &self.originator)
            .finish_non_exhaustive()
    }
}

/// The address a remote forward asks the server to listen on: OpenSSH's `localhost` for the
/// loopback address, and the empty string for every address (RFC 4254 §7.1).
fn remote_bind(bind: Bind) -> &'static str {
    match bind {
        Bind::Localhost => "localhost",
        Bind::AllInterfaces => "",
    }
}

/// What a channel-open refusal means, in words for the user.
fn refusal(failure: &ChannelOpenFailure) -> String {
    match failure {
        ChannelOpenFailure::AdministrativelyProhibited => {
            "the server does not allow port forwarding".to_owned()
        }
        ChannelOpenFailure::ConnectFailed => "the connection was refused or timed out".to_owned(),
        ChannelOpenFailure::UnknownChannelType => {
            "the server does not support port forwarding".to_owned()
        }
        ChannelOpenFailure::ResourceShortage => "the server is out of resources".to_owned(),
        ChannelOpenFailure::Other { reason, .. } => reason.clone(),
    }
}

impl Session {
    /// Opens a channel to `host:port` from the server (`direct-tcpip`), on behalf of a program
    /// at `originator` on this machine.
    async fn open_direct(
        &self,
        host: &str,
        port: u16,
        originator: SocketAddr,
    ) -> Result<Channel<Msg>, SshError> {
        self.handle()
            .channel_open_direct_tcpip(
                host,
                u32::from(port),
                originator.ip().to_string(),
                u32::from(originator.port()),
            )
            .await
            .map_err(|error| match error {
                russh::Error::ChannelOpenFailure(failure) => SshError::ForwardRefused {
                    destination: format!("{}:{port}", bracketed(host)),
                    reason: refusal(&failure),
                },
                other => other.into(),
            })
    }

    /// Asks the server to listen on `listen` and returns the connections that arrive there.
    async fn listen_remote(&self, listen: Listen) -> Result<mpsc::Receiver<Incoming>, SshError> {
        let port = u32::from(listen.port.get());
        let (sender, incoming) = mpsc::channel(INCOMING_CAPACITY);
        // In place before asking, so a connection that arrives straight away finds its way.
        self.routes().add(port, sender);
        match self
            .handle()
            .tcpip_forward(remote_bind(listen.bind), port)
            .await
        {
            Ok(_) => Ok(incoming),
            Err(error) => {
                self.routes().remove(port);
                Err(match error {
                    russh::Error::RequestDenied => SshError::RemoteListenRefused {
                        port: listen.port.get(),
                    },
                    other => other.into(),
                })
            }
        }
    }

    /// Asks the server to stop listening on `listen`. A failure is only logged: the listener
    /// goes with the connection anyway.
    async fn stop_listening(&self, listen: Listen) {
        let port = u32::from(listen.port.get());
        self.routes().remove(port);
        if let Err(error) = self
            .handle()
            .cancel_tcpip_forward(remote_bind(listen.bind), port)
            .await
        {
            tracing::debug!(%error, "the server did not stop listening; it will when the connection ends");
        }
    }
}

/// `host` ready to have `:port` appended: an IPv6 address gets brackets.
fn bracketed(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn remote_bind_addresses() {
        assert_eq!(remote_bind(Bind::Localhost), "localhost");
        assert_eq!(remote_bind(Bind::AllInterfaces), "");
    }

    #[test]
    fn refusals_are_explained() {
        assert_eq!(
            refusal(&ChannelOpenFailure::AdministrativelyProhibited),
            "the server does not allow port forwarding"
        );
        assert_eq!(
            refusal(&ChannelOpenFailure::Other {
                code: 9,
                reason: "nope".into()
            }),
            "nope"
        );
    }

    #[test]
    fn hosts_are_bracketed_for_ports() {
        assert_eq!(bracketed("db"), "db");
        assert_eq!(bracketed("::1"), "[::1]");
    }

    #[test]
    fn finished_statuses() {
        assert!(ForwardStatus::Stopped.is_finished());
        assert!(ForwardStatus::Failed("x".into()).is_finished());
        assert!(!ForwardStatus::Waiting.is_finished());
        assert!(!ForwardStatus::Active.is_finished());
    }

    #[test]
    fn connections_are_counted_while_they_live() {
        let counters = Counters::default();
        {
            let _a = Tracked::new(&counters);
            let _b = Tracked::new(&counters);
            assert_eq!(counters.snapshot().active_connections, 2);
        }
        let stats = counters.snapshot();
        assert_eq!((stats.active_connections, stats.total_connections), (0, 2));
        counters.failed(&SshError::NotConnected);
        assert_eq!(
            counters.snapshot().last_error.as_deref(),
            Some("the connection is not open")
        );
    }
}
