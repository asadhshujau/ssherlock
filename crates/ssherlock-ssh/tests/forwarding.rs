//! Port forwarding (FWD-01…03, 06, 07): local, remote and SOCKS forwards over a
//! forwarding-only connection, against an in-process russh server (runs everywhere) and the
//! Docker OpenSSH server (skipped without Docker).
#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: failing loudly is the point"
)]

#[path = "docker/harness.rs"]
mod harness;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::fixture;
use russh::server::{self, Auth, Msg};
use russh::{Channel, ChannelOpenFailure};
use ssherlock_core::Port;
use ssherlock_core::forwarding::{Destination, Forward, Listen};
use ssherlock_ssh::connection::{DisconnectReason, KeepAlive, ReconnectPolicy};
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::HostKeyDecision;
use ssherlock_ssh::{
    AuthMethod, ConnectConfig, ConnectionHandle, ConnectionState, ForwardStatus, HostKeys,
    RemoteShellConfig, RunningForward, SessionEvent, spawn_forwarding,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(10);

// ── An in-process SSH server that forwards ─────────────────────────────────────────────────

/// An SSH server in this process that lets anyone in and forwards like OpenSSH does: it
/// connects `direct-tcpip` channels to their destination, and listens on 127.0.0.1 for
/// `tcpip-forward` requests. Remote forwards on `refused_port` are refused.
struct InProcessServer {
    addr: SocketAddr,
    sessions: Arc<Mutex<Vec<server::Handle>>>,
    _accept: JoinHandle<()>,
}

impl InProcessServer {
    async fn start(refused_port: Option<u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Arc::new(server::Config {
            keys: vec![russh::keys::load_secret_key(fixture("id_ed25519"), None).unwrap()],
            auth_rejection_time: Duration::ZERO,
            ..server::Config::default()
        });
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::clone(&sessions);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = Forwarder {
                    refused_port: refused_port.map(u32::from),
                    listeners: Arc::new(Mutex::new(HashMap::new())),
                };
                if let Ok(running) = server::run_stream(Arc::clone(&config), stream, handler).await
                {
                    registry.lock().unwrap().push(running.handle());
                    tokio::spawn(running);
                }
            }
        });
        Self {
            addr,
            sessions,
            _accept: accept,
        }
    }

    /// Ends every session, as a server restart or a network drop would.
    async fn drop_sessions(&self) {
        let sessions: Vec<server::Handle> = std::mem::take(&mut *self.sessions.lock().unwrap());
        for session in sessions {
            let _ = session
                .disconnect(
                    russh::Disconnect::ByApplication,
                    String::new(),
                    String::new(),
                )
                .await;
        }
    }
}

struct Forwarder {
    refused_port: Option<u32>,
    /// Remote forwards' listeners by port; stopped when the session ends.
    listeners: Arc<Mutex<HashMap<u32, JoinHandle<()>>>>,
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        for (_, listener) in self.listeners.lock().unwrap().drain() {
            listener.abort();
        }
    }
}

impl server::Handler for Forwarder {
    type Error = russh::Error;

    fn auth_none(&mut self, _user: &str) -> impl Future<Output = Result<Auth, Self::Error>> + Send {
        std::future::ready(Ok(Auth::Accept))
    }

    fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let target = (
            host_to_connect.to_owned(),
            u16::try_from(port_to_connect).unwrap(),
        );
        tokio::spawn(async move {
            match TcpStream::connect(target).await {
                Ok(mut socket) => {
                    reply.accept().await;
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
        std::future::ready(Ok(()))
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        if Some(*port) == self.refused_port {
            return Ok(false);
        }
        let Ok(listener) = TcpListener::bind(("127.0.0.1", u16::try_from(*port).unwrap())).await
        else {
            return Ok(false);
        };
        let handle = session.handle();
        let (address, port) = (address.to_owned(), *port);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, peer)) = listener.accept().await {
                let handle = handle.clone();
                let address = address.clone();
                tokio::spawn(async move {
                    let channel = handle
                        .channel_open_forwarded_tcpip(
                            address,
                            port,
                            peer.ip().to_string(),
                            u32::from(peer.port()),
                        )
                        .await;
                    if let Ok(channel) = channel {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                    }
                });
            }
        });
        self.listeners.lock().unwrap().insert(port, task);
        Ok(true)
    }

    fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        port: u32,
        _session: &mut server::Session,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        let listener = self.listeners.lock().unwrap().remove(&port);
        std::future::ready(Ok(listener.map(|l| l.abort()).is_some()))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────────────────

/// A TCP server on this machine that echoes everything back, standing in for a database or a
/// web server.
async fn echo_server() -> (Port, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = Port::new(listener.local_addr().unwrap().port()).unwrap();
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    (port, task)
}

/// A port that was free a moment ago.
async fn free_port() -> Port {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    Port::new(probe.local_addr().unwrap().port()).unwrap()
}

fn localhost(port: Port) -> Destination {
    Destination {
        host: "127.0.0.1".parse().unwrap(),
        port,
    }
}

/// A forwarding-only connection with fast keep-alives and retries, logging in as `user` with
/// `methods`, and its connection states.
fn connect(
    addr: SocketAddr,
    user: &str,
    methods: Vec<AuthMethod>,
    test: &str,
) -> (ConnectionHandle, mpsc::UnboundedReceiver<ConnectionState>) {
    let dir = std::env::temp_dir().join(format!(
        "ssherlock-forwarding-{test}-{}",
        std::process::id()
    ));
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let connect = ConnectConfig::new(addr.ip().to_string(), user)
        .with_port(addr.port())
        .with_connect_timeout(Duration::from_secs(2));
    let mut config = RemoteShellConfig::new(connect, methods);
    config.keep_alive = KeepAlive {
        interval: Duration::from_millis(300),
        max_missed: 2,
    };
    config.reconnect = ReconnectPolicy {
        max_attempts: 20,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
    };
    let (events_tx, mut events) = mpsc::channel(16);
    let (states_tx, states) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::HostKey(prompt) => prompt.answer(HostKeyDecision::Trust),
                SessionEvent::State(state) => {
                    let _ = states_tx.send(state);
                }
                SessionEvent::Prompt(prompt) => panic!("unexpected prompt {:?}", prompt.kind()),
                SessionEvent::Banner(_) => {}
            }
        }
    });
    let handle = spawn_forwarding(
        config,
        host_keys,
        events_tx,
        &tokio::runtime::Handle::current(),
    );
    (handle, states)
}

fn connect_in_process(server: &InProcessServer, test: &str) -> ConnectionHandle {
    connect(server.addr, "tester", vec![AuthMethod::Password], test).0
}

/// Waits until the forward's status is `wanted`, failing on any finished status that isn't.
async fn wait_status(forward: &RunningForward, wanted: &ForwardStatus) {
    let mut watch = forward.watch();
    timeout(LIMIT, async {
        loop {
            let status = watch.borrow_and_update().clone();
            if status == *wanted {
                return;
            }
            assert!(
                !status.is_finished(),
                "{} ended {status:?} waiting for {wanted:?}",
                forward.forward()
            );
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for {wanted:?}; it is {:?}",
            forward.status()
        )
    });
}

/// Waits for a finished status and returns it.
async fn wait_finished(forward: &RunningForward) -> ForwardStatus {
    let mut watch = forward.watch();
    timeout(LIMIT, async {
        loop {
            let status = watch.borrow_and_update().clone();
            if status.is_finished() {
                return status;
            }
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; it is {:?}", forward.status()))
}

/// Sends `message` through `port` on this machine and checks it comes back.
async fn round_trip(port: Port, message: &[u8]) {
    let mut socket = timeout(LIMIT, TcpStream::connect(("127.0.0.1", port.get())))
        .await
        .unwrap()
        .unwrap();
    socket.write_all(message).await.unwrap();
    let mut answer = vec![0_u8; message.len()];
    timeout(LIMIT, socket.read_exact(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("no echo through port {port}"))
        .unwrap();
    assert_eq!(answer, message);
}

/// Opens a SOCKS5 connection through the proxy on `proxy` to `host:port`; returns the reply
/// code and, on success, the stream.
async fn socks_connect(proxy: Port, host: &str, port: u16) -> (u8, TcpStream) {
    let mut socket = TcpStream::connect(("127.0.0.1", proxy.get()))
        .await
        .unwrap();
    socket.write_all(&[5, 1, 0]).await.unwrap();
    let mut choice = [0_u8; 2];
    socket.read_exact(&mut choice).await.unwrap();
    assert_eq!(choice, [5, 0]);
    let mut request = vec![5, 1, 0, 3, u8::try_from(host.len()).unwrap()];
    request.extend(host.as_bytes());
    request.extend(port.to_be_bytes());
    socket.write_all(&request).await.unwrap();
    let mut reply = [0_u8; 10];
    timeout(LIMIT, socket.read_exact(&mut reply))
        .await
        .unwrap()
        .unwrap();
    (reply[1], socket)
}

/// Waits until `check` passes on the forward's stats; counters update as bytes move.
async fn wait_stats(
    forward: &RunningForward,
    check: impl Fn(&ssherlock_ssh::ForwardStats) -> bool,
) {
    timeout(LIMIT, async {
        while !check(&forward.stats()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("stats never matched: {:?}", forward.stats()));
}

// ── In-process tests ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_local_forward_carries_connections_and_counts_them() {
    let server = InProcessServer::start(None).await;
    let (echo, _echo) = echo_server().await;
    let connection = connect_in_process(&server, "local");
    let port = free_port().await;
    let forward = connection.start_forward(Forward::Local {
        listen: Listen::localhost(port),
        destination: localhost(echo),
    });
    wait_status(&forward, &ForwardStatus::Active).await;

    round_trip(port, b"hello through -L").await;
    round_trip(port, b"again").await;
    wait_stats(&forward, |s| {
        s.uploaded == 21 && s.downloaded == 21 && s.total_connections == 2
    })
    .await;
    wait_stats(&forward, |s| s.active_connections == 0).await;

    drop(forward);
    // Stopped: the port is free again.
    timeout(LIMIT, async {
        while TcpListener::bind(("127.0.0.1", port.get())).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_dynamic_forward_is_a_socks5_proxy() {
    let server = InProcessServer::start(None).await;
    let (echo, _echo) = echo_server().await;
    let connection = connect_in_process(&server, "dynamic");
    let proxy = free_port().await;
    let forward = connection.start_forward(Forward::Dynamic {
        listen: Listen::localhost(proxy),
    });
    wait_status(&forward, &ForwardStatus::Active).await;

    let (code, mut socket) = socks_connect(proxy, "127.0.0.1", echo.get()).await;
    assert_eq!(code, 0);
    socket.write_all(b"via socks").await.unwrap();
    let mut answer = [0_u8; 9];
    timeout(LIMIT, socket.read_exact(&mut answer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&answer, b"via socks");

    // Nothing listens there: the proxy says so, and the forward remembers why.
    let closed = free_port().await;
    let (code, _) = socks_connect(proxy, "127.0.0.1", closed.get()).await;
    assert_eq!(code, 1);
    wait_stats(&forward, |s| {
        s.last_error.as_deref().is_some_and(|e| {
            e.contains(&format!("127.0.0.1:{closed}")) && e.contains("refused or timed out")
        })
    })
    .await;
}

#[tokio::test]
async fn a_remote_forward_carries_connections_back() {
    let server = InProcessServer::start(None).await;
    let (echo, _echo) = echo_server().await;
    let connection = connect_in_process(&server, "remote");
    // The in-process server listens on this machine, which is also "the server".
    let server_port = free_port().await;
    let forward = connection.start_forward(Forward::Remote {
        listen: Listen::localhost(server_port),
        destination: localhost(echo),
    });
    wait_status(&forward, &ForwardStatus::Active).await;
    round_trip(server_port, b"hello through -R").await;
    wait_stats(&forward, |s| s.downloaded == 16 && s.uploaded == 16).await;

    // A destination that doesn't answer is refused, and remembered.
    let gone = free_port().await;
    let unreachable = connection.start_forward(Forward::Remote {
        listen: Listen::localhost(free_port().await),
        destination: localhost(gone),
    });
    wait_status(&unreachable, &ForwardStatus::Active).await;
    let listen = unreachable.forward().listen();
    let mut socket = TcpStream::connect(("127.0.0.1", listen.port.get()))
        .await
        .unwrap();
    let mut buffer = [0_u8; 1];
    // The server closes the connection once the channel is refused.
    let read = timeout(LIMIT, socket.read(&mut buffer)).await.unwrap();
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    wait_stats(&unreachable, |s| {
        s.last_error
            .as_deref()
            .is_some_and(|e| e.starts_with(&format!("could not connect to 127.0.0.1:{gone}")))
    })
    .await;

    // Stopping asks the server to stop listening.
    drop(forward);
    timeout(LIMIT, async {
        while TcpListener::bind(("127.0.0.1", server_port.get()))
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn forwards_that_cannot_listen_fail_with_the_reason() {
    let refused = free_port().await;
    let server = InProcessServer::start(Some(refused.get())).await;
    let connection = connect_in_process(&server, "fail");

    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let taken_port = Port::new(taken.local_addr().unwrap().port()).unwrap();
    let local = connection.start_forward(Forward::Local {
        listen: Listen::localhost(taken_port),
        destination: localhost(refused),
    });
    let ForwardStatus::Failed(reason) = wait_finished(&local).await else {
        panic!("expected a failure");
    };
    assert!(
        reason.starts_with(&format!("could not listen on 127.0.0.1:{taken_port}: ")),
        "{reason}"
    );

    let remote = connection.start_forward(Forward::Remote {
        listen: Listen::localhost(refused),
        destination: localhost(refused),
    });
    assert_eq!(
        wait_finished(&remote).await,
        ForwardStatus::Failed(format!("the server refused to listen on port {refused}"))
    );
}

#[tokio::test]
async fn forwards_wait_out_a_reconnect_and_start_again() {
    let server = InProcessServer::start(None).await;
    let (echo, _echo) = echo_server().await;
    let (connection, mut states) = connect(
        server.addr,
        "tester",
        vec![AuthMethod::Password],
        "reconnect",
    );
    let (local_port, server_port) = (free_port().await, free_port().await);
    let local = connection.start_forward(Forward::Local {
        listen: Listen::localhost(local_port),
        destination: localhost(echo),
    });
    let remote = connection.start_forward(Forward::Remote {
        listen: Listen::localhost(server_port),
        destination: localhost(echo),
    });
    wait_status(&local, &ForwardStatus::Active).await;
    wait_status(&remote, &ForwardStatus::Active).await;
    round_trip(local_port, b"before").await;
    round_trip(server_port, b"before").await;

    server.drop_sessions().await;
    timeout(LIMIT, async {
        while !matches!(
            states.recv().await,
            Some(ConnectionState::Reconnecting { .. })
        ) {}
    })
    .await
    .unwrap();
    // Each comes back on the new session: the local port stayed open, the remote one was
    // asked for again.
    wait_status(&local, &ForwardStatus::Active).await;
    wait_status(&remote, &ForwardStatus::Active).await;
    round_trip(local_port, b"after").await;
    round_trip(server_port, b"after").await;
}

#[tokio::test]
async fn dropping_the_handle_ends_a_forwarding_only_connection() {
    let server = InProcessServer::start(None).await;
    let (connection, mut states) =
        connect(server.addr, "tester", vec![AuthMethod::Password], "close");
    let forward = connection.start_forward(Forward::Dynamic {
        listen: Listen::localhost(free_port().await),
    });
    wait_status(&forward, &ForwardStatus::Active).await;
    drop(connection);
    let end = timeout(LIMIT, async {
        loop {
            if let Some(ConnectionState::Disconnected(reason)) = states.recv().await {
                return reason;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(end, DisconnectReason::Closed);
    // The forward ends with its connection.
    assert_eq!(wait_finished(&forward).await, ForwardStatus::Stopped);
}

// ── Against OpenSSH (Docker) ───────────────────────────────────────────────────────────────

/// The exit check's shape against a real sshd: a local forward reaches a service on the
/// server (its own sshd, standing in for a database), a SOCKS proxy does the same, and a
/// remote forward carries connections made on the server back here.
#[tokio::test]
async fn forwards_work_against_openssh() {
    let Some(server) = harness::TestServer::start("forwards_work_against_openssh") else {
        return;
    };
    let (connection, _states) = connect(
        server.addr(),
        harness::USER,
        vec![AuthMethod::KeyFile(fixture("id_ed25519"))],
        "openssh",
    );
    let ssh_on_server = Destination {
        host: "127.0.0.1".parse().unwrap(),
        port: Port::SSH,
    };

    // -L: the server's own sshd answers with its banner.
    let local_port = free_port().await;
    let local = connection.start_forward(Forward::Local {
        listen: Listen::localhost(local_port),
        destination: ssh_on_server.clone(),
    });
    wait_status(&local, &ForwardStatus::Active).await;
    let mut socket = TcpStream::connect(("127.0.0.1", local_port.get()))
        .await
        .unwrap();
    let mut banner = [0_u8; 8];
    timeout(LIMIT, socket.read_exact(&mut banner))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&banner, b"SSH-2.0-");

    // -D: the same through SOCKS5, by name, resolved on the server.
    let proxy = free_port().await;
    let dynamic = connection.start_forward(Forward::Dynamic {
        listen: Listen::localhost(proxy),
    });
    wait_status(&dynamic, &ForwardStatus::Active).await;
    let (code, mut socket) = socks_connect(proxy, "localhost", 22).await;
    assert_eq!(code, 0);
    timeout(LIMIT, socket.read_exact(&mut banner))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&banner, b"SSH-2.0-");

    // -R: the server listens on 127.0.0.1:2345 and sends what connects back to our echo
    // server. A second -L reaches that port from here, so the round trip goes out through
    // one forward and back through the other.
    let (echo, _echo) = echo_server().await;
    let on_server = Port::new(2345).unwrap();
    let remote = connection.start_forward(Forward::Remote {
        listen: Listen::localhost(on_server),
        destination: localhost(echo),
    });
    wait_status(&remote, &ForwardStatus::Active).await;
    let loop_port = free_port().await;
    let to_remote = connection.start_forward(Forward::Local {
        listen: Listen::localhost(loop_port),
        destination: Destination {
            host: "127.0.0.1".parse().unwrap(),
            port: on_server,
        },
    });
    wait_status(&to_remote, &ForwardStatus::Active).await;
    round_trip(loop_port, b"out with -L, back with -R").await;
    wait_stats(&remote, |s| s.total_connections == 1 && s.uploaded == 25).await;
}
