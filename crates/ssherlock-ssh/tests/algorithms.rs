//! Algorithm preferences and compression against the Docker OpenSSH server.
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
use ssherlock_ssh::{
    AlgorithmPreferences, AuthMethod, Compression, ConnectConfig, HostKeys, Session, SessionEvent,
    SshError,
};
use ssherlock_terminal::{PtyInput, PtyIo, PtyOutput, TerminalSize};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Connects with `config`, trusting the host key, and logs in. If connecting fails, also returns
/// the algorithms of the host keys the user was asked about (none, for a failed negotiation).
async fn connect(config: ConnectConfig, test: &str) -> (Result<Session, SshError>, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("ssherlock-alg-{test}-{}", std::process::id()));
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let (tx, mut rx) = mpsc::channel(4);
    let seen = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(event) = rx.recv().await {
            if let SessionEvent::HostKey(prompt) = event {
                seen.push(prompt.key().algorithm().as_str().to_owned());
                prompt.answer(HostKeyDecision::Trust);
            }
        }
        seen
    });
    let result = Session::connect(config, host_keys, tx).await;
    let mut session = match result {
        Ok(session) => session,
        Err(e) => return (Err(e), seen.await.unwrap()),
    };
    session
        .authenticate(&[AuthMethod::KeyFile(fixture("id_ed25519"))])
        .await
        .unwrap();
    // The listener lives as long as the session's event sender; it's not needed any more.
    seen.abort();
    (Ok(session), Vec::new())
}

async fn echo_round_trip(session: &Session) {
    let mut io: PtyIo = session.open_shell(TerminalSize::new(80, 24)).await.unwrap();
    io.input
        .send(PtyInput::Write(b"echo works-$((40+2))\n".to_vec()))
        .await
        .unwrap();
    let mut seen = String::new();
    timeout(Duration::from_secs(10), async {
        while !seen.contains("works-42") {
            match io.output.recv().await {
                Some(PtyOutput::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("unexpected {other:?} after {seen:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; got {seen:?}"));
}

fn base(server: &TestServer) -> ConnectConfig {
    ConnectConfig::new("127.0.0.1", harness::USER).with_port(server.addr().port())
}

#[tokio::test]
async fn chosen_algorithms_are_used() {
    let Some(server) = TestServer::start("chosen_algorithms_are_used") else {
        return;
    };
    let algorithms = AlgorithmPreferences {
        kex: vec!["curve25519-sha256".into()],
        host_key: vec!["ecdsa-sha2-nistp256".into()],
        cipher: vec!["aes128-ctr".into()],
        mac: vec!["hmac-sha2-256".into()],
    };
    // Track the host key prompt separately: it shows which host key type was negotiated.
    let dir = std::env::temp_dir().join(format!("ssherlock-alg-chosen-{}", std::process::id()));
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let (tx, mut rx) = mpsc::channel(4);
    let (key_tx, mut key_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let SessionEvent::HostKey(prompt) = event {
                let _ = key_tx.send(prompt.key().algorithm().as_str().to_owned());
                prompt.answer(HostKeyDecision::Trust);
            }
        }
    });
    let mut session = Session::connect(base(&server).with_algorithms(algorithms), host_keys, tx)
        .await
        .unwrap();
    assert_eq!(key_rx.recv().await.as_deref(), Some("ecdsa-sha2-nistp256"));
    session
        .authenticate(&[AuthMethod::KeyFile(fixture("id_ed25519"))])
        .await
        .unwrap();
    echo_round_trip(&session).await;
}

#[tokio::test]
async fn compression_works_end_to_end() {
    let Some(server) = TestServer::start("compression_works_end_to_end") else {
        return;
    };
    let (session, _) = connect(base(&server).with_compression(Compression::On), "zlib").await;
    echo_round_trip(&session.unwrap()).await;
}

#[tokio::test]
async fn an_algorithm_the_server_lacks_fails_cleanly() {
    let Some(server) = TestServer::start("an_algorithm_the_server_lacks_fails_cleanly") else {
        return;
    };
    // russh can do AES-CBC, but OpenSSH stopped offering CBC ciphers by default long ago.
    let algorithms = AlgorithmPreferences {
        cipher: vec!["aes256-cbc".into()],
        ..AlgorithmPreferences::default()
    };
    let (result, prompts) = connect(base(&server).with_algorithms(algorithms), "no-common").await;
    assert!(matches!(result, Err(SshError::Protocol(_))), "{result:?}");
    assert_eq!(
        prompts,
        Vec::<String>::new(),
        "no host key question before a failed negotiation"
    );
}

#[tokio::test]
async fn a_typo_fails_before_connecting() {
    // No server: the preference is refused before any network traffic.
    let algorithms = AlgorithmPreferences {
        cipher: vec!["aes256-gmc".into()],
        ..AlgorithmPreferences::default()
    };
    let config = ConnectConfig::new("192.0.2.1", "u").with_algorithms(algorithms);
    let (result, _) = connect(config, "typo").await;
    match result {
        Err(SshError::UnknownAlgorithm { kind, name }) => {
            assert_eq!((kind, name.as_str()), ("cipher", "aes256-gmc"));
        }
        other => panic!("expected UnknownAlgorithm, got {other:?}"),
    }
}
