//! Interactive shells over SSH against the Docker OpenSSH server: bytes both ways, resizes,
//! exit status, and a real `TerminalHandle` rendering a remote shell.
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
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::HostKeyDecision;
use ssherlock_ssh::{AuthMethod, ConnectConfig, HostKeys, Session, SessionEvent};
use ssherlock_terminal::{
    PtyExit, PtyInput, PtyIo, PtyOutput, TerminalConfig, TerminalEvent, TerminalHandle,
    TerminalSize,
};
use tokio::sync::mpsc;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(10);

/// Connects, trusts the host key and logs in with the Ed25519 fixture key.
async fn logged_in(server: &TestServer, test: &str) -> Session {
    let dir = std::env::temp_dir().join(format!("ssherlock-shell-{test}-{}", std::process::id()));
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let (tx, mut rx) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let SessionEvent::HostKey(prompt) = event {
                prompt.answer(HostKeyDecision::Trust);
            }
        }
    });
    let config = ConnectConfig::new("127.0.0.1", harness::USER).with_port(server.addr().port());
    let mut session = Session::connect(config, host_keys, tx).await.unwrap();
    session
        .authenticate(&[AuthMethod::KeyFile(fixture("id_ed25519"))])
        .await
        .unwrap();
    session
}

/// Reads shell output until it contains `needle`; returns everything read.
async fn read_until(io: &mut PtyIo, needle: &str) -> String {
    let mut seen = String::new();
    timeout(LIMIT, async {
        while !seen.contains(needle) {
            match io.output.recv().await {
                Some(PtyOutput::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => {
                    panic!("expected output containing {needle:?}, got {other:?} after {seen:?}")
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {seen:?}"));
    seen
}

async fn run(io: &PtyIo, command: &str) {
    io.input
        .send(PtyInput::Write(format!("{command}\n").into_bytes()))
        .await
        .unwrap();
}

#[tokio::test]
async fn shell_runs_commands_resizes_and_reports_exit_status() {
    let Some(server) = TestServer::start("shell_runs_commands_resizes_and_reports_exit_status")
    else {
        return;
    };
    let session = logged_in(&server, "basic").await;
    let mut io = session
        .open_shell(TerminalSize::new(100, 30))
        .await
        .unwrap();

    // Arithmetic proves the shell ran it, not that the echo of our typing came back.
    run(&io, "echo answer-$((6*7)); echo term=$TERM").await;
    let out = read_until(&mut io, "term=xterm-256color").await;
    assert!(out.contains("answer-42"), "{out:?}");

    run(&io, "echo size=$(stty size | tr ' ' x)").await;
    read_until(&mut io, "size=30x100").await;
    io.input
        .send(PtyInput::Resize(TerminalSize::new(120, 40)))
        .await
        .unwrap();
    run(&io, "echo size=$(stty size | tr ' ' x)").await;
    read_until(&mut io, "size=40x120").await;

    run(&io, "exit 3").await;
    let exit = timeout(LIMIT, async {
        loop {
            match io.output.recv().await {
                Some(PtyOutput::Exited(exit)) => return exit,
                Some(PtyOutput::Data(_)) => {}
                None => panic!("output ended without Exited"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(exit, PtyExit::Code(3));
    assert!(io.output.recv().await.is_none(), "nothing follows Exited");
}

#[tokio::test]
async fn dropping_the_terminal_closes_only_its_channel() {
    let Some(server) = TestServer::start("dropping_the_terminal_closes_only_its_channel") else {
        return;
    };
    let session = logged_in(&server, "drop").await;
    let first = session.open_shell(TerminalSize::new(80, 24)).await.unwrap();
    drop(first);
    // The connection stays up and can open another shell.
    let mut second = session.open_shell(TerminalSize::new(80, 24)).await.unwrap();
    run(&second, "echo still-$((40+2))").await;
    read_until(&mut second, "still-42").await;
    assert!(!session.is_closed());
}

#[tokio::test]
async fn terminal_renders_a_remote_shell() {
    let Some(server) = TestServer::start("terminal_renders_a_remote_shell") else {
        return;
    };
    let session = logged_in(&server, "render").await;
    let size = TerminalSize::new(60, 10);
    let io = session.open_shell(size).await.unwrap();
    let (terminal, mut events) = TerminalHandle::spawn(
        io,
        &TerminalConfig::default(),
        size,
        &tokio::runtime::Handle::current(),
    );

    terminal
        .write(b"clear; echo rendered-$((20+22))\n".to_vec())
        .unwrap();
    let mut snapshots = terminal.subscribe();
    timeout(LIMIT, async {
        loop {
            let screen: String = snapshots
                .borrow_and_update()
                .lines()
                .map(|row| row.iter().map(|c| c.ch).collect::<String>() + "\n")
                .collect();
            if screen.contains("rendered-42") {
                return;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .expect("remote output never reached the terminal grid");

    terminal.write(b"exit 0\n".to_vec()).unwrap();
    let exited = timeout(LIMIT, async {
        loop {
            match events.recv().await {
                Some(TerminalEvent::Exited(exit)) => return exit,
                Some(_) => {}
                None => panic!("terminal events ended without Exited"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(exited, PtyExit::Code(0));
}
