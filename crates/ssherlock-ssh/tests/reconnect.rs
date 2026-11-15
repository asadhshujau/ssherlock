//! The reconnecting remote shell against the Docker OpenSSH server: states, latency, surviving
//! a frozen network, and finishing cleanly.
#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: failing loudly is the point"
)]

#[path = "docker/harness.rs"]
mod harness;

use std::time::Duration;

use harness::{TestServer, fixture};
use ssherlock_ssh::connection::{DisconnectReason, KeepAlive, ReconnectPolicy};
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::HostKeyDecision;
use ssherlock_ssh::{
    AuthMethod, ConnectConfig, ConnectionState, HostKeys, RemoteShellConfig, SessionEvent,
    spawn_shell,
};
use ssherlock_terminal::{PtyExit, PtyInput, PtyIo, PtyOutput, TerminalSize};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Starts a remote shell with fast keep-alives and retries; returns its `PtyIo` and its states.
fn start(server_port: u16, test: &str) -> (PtyIo, mpsc::UnboundedReceiver<ConnectionState>) {
    let dir =
        std::env::temp_dir().join(format!("ssherlock-reconnect-{test}-{}", std::process::id()));
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let connect = ConnectConfig::new("127.0.0.1", harness::USER)
        .with_port(server_port)
        .with_connect_timeout(Duration::from_secs(1));
    let mut config =
        RemoteShellConfig::new(connect, vec![AuthMethod::KeyFile(fixture("id_ed25519"))]);
    config.keep_alive = KeepAlive {
        interval: Duration::from_millis(300),
        max_missed: 2,
    };
    config.reconnect = ReconnectPolicy {
        max_attempts: 30,
        initial_delay: Duration::from_millis(200),
        max_delay: Duration::from_secs(1),
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
    let io = spawn_shell(
        config,
        host_keys,
        TerminalSize::new(80, 24),
        events_tx,
        &tokio::runtime::Handle::current(),
    );
    (io, states)
}

/// Waits for a state matching `wanted`, returning every state seen up to it.
async fn wait_for(
    states: &mut mpsc::UnboundedReceiver<ConnectionState>,
    limit: Duration,
    wanted: impl Fn(&ConnectionState) -> bool,
) -> Vec<ConnectionState> {
    let mut seen = Vec::new();
    timeout(limit, async {
        loop {
            let state = states.recv().await.expect("state stream ended");
            let done = wanted(&state);
            seen.push(state);
            if done {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; states so far: {seen:?}"));
    seen
}

async fn read_until(io: &mut PtyIo, needle: &str) {
    let mut seen = String::new();
    timeout(Duration::from_secs(10), async {
        while !seen.contains(needle) {
            match io.output.recv().await {
                Some(PtyOutput::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("expected {needle:?}, got {other:?} after {seen:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {seen:?}"));
}

async fn run(io: &PtyIo, command: &str) {
    io.input
        .send(PtyInput::Write(format!("{command}\n").into_bytes()))
        .await
        .unwrap();
}

fn is_connected(state: &ConnectionState) -> bool {
    matches!(state, ConnectionState::Connected { .. })
}

#[tokio::test]
async fn survives_a_frozen_network_and_reports_latency() {
    let Some(server) = TestServer::start("survives_a_frozen_network_and_reports_latency") else {
        return;
    };
    let (mut io, mut states) = start(server.addr().port(), "frozen");

    let first = wait_for(&mut states, Duration::from_secs(15), is_connected).await;
    assert_eq!(
        first.get(..2),
        Some(&[ConnectionState::Connecting, ConnectionState::Authenticating][..]),
        "{first:?}"
    );
    wait_for(&mut states, Duration::from_secs(5), |s| {
        matches!(s, ConnectionState::Connected { latency: Some(_) })
    })
    .await;
    run(&io, "echo before-$((40+2))").await;
    read_until(&mut io, "before-42").await;

    server.pause();
    wait_for(&mut states, Duration::from_secs(10), |s| {
        matches!(s, ConnectionState::Reconnecting { .. })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    server.unpause();

    let back = wait_for(&mut states, Duration::from_secs(30), is_connected).await;
    assert!(back.contains(&ConnectionState::Authenticating), "{back:?}");
    run(&io, "echo after-$((40+2))").await;
    read_until(&mut io, "after-42").await;

    run(&io, "exit 5").await;
    let end = wait_for(&mut states, Duration::from_secs(10), |s| {
        matches!(s, ConnectionState::Disconnected(_))
    })
    .await;
    assert_eq!(
        end.last(),
        Some(&ConnectionState::Disconnected(
            DisconnectReason::ShellExited(PtyExit::Code(5))
        ))
    );
    let exit = timeout(Duration::from_secs(5), async {
        loop {
            match io.output.recv().await {
                Some(PtyOutput::Exited(exit)) => return exit,
                Some(PtyOutput::Data(_)) => {}
                None => panic!("no Exited"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(exit, PtyExit::Code(5));
}

#[tokio::test]
async fn closing_the_terminal_disconnects() {
    let Some(server) = TestServer::start("closing_the_terminal_disconnects") else {
        return;
    };
    let (io, mut states) = start(server.addr().port(), "close");
    wait_for(&mut states, Duration::from_secs(15), is_connected).await;
    drop(io);
    let end = wait_for(&mut states, Duration::from_secs(5), |s| {
        matches!(s, ConnectionState::Disconnected(_))
    })
    .await;
    assert_eq!(
        end.last(),
        Some(&ConnectionState::Disconnected(DisconnectReason::Closed))
    );
}

#[tokio::test]
async fn a_failed_first_connection_is_not_retried() {
    // No Docker needed: nothing listens on this port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (mut io, mut states) = start(port, "refused");
    let end = wait_for(&mut states, Duration::from_secs(5), |s| {
        matches!(s, ConnectionState::Disconnected(_))
    })
    .await;
    match end.last() {
        Some(ConnectionState::Disconnected(DisconnectReason::Failed(message))) => {
            assert!(
                message.starts_with(&format!("could not connect to 127.0.0.1:{port}: ")),
                "{message}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(
        !end.iter()
            .any(|s| matches!(s, ConnectionState::Reconnecting { .. })),
        "{end:?}"
    );
    assert_eq!(
        io.output.recv().await,
        Some(PtyOutput::Exited(PtyExit::Unknown))
    );
}
