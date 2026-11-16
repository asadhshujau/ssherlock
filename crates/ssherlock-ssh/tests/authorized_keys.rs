//! Adding a key to `authorized_keys` on the Docker OpenSSH server (KEY-07), then logging in
//! with it.
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

use harness::TestServer;
use ssherlock_core::SecretString;
use ssherlock_core::keychain::KeyKind;
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::HostKeyDecision;
use ssherlock_ssh::{
    AuthMethod, ConnectConfig, HostKeys, KeyInstall, RemoteShellConfig, Session, SessionEvent,
    SshError, StoredKey, keys, spawn_shell,
};
use ssherlock_terminal::TerminalSize;
use tokio::sync::mpsc;

/// Connects, trusting the host key, and logs in with `methods` (no prompts answered).
async fn session(server: &TestServer, methods: &[AuthMethod]) -> Result<Session, SshError> {
    let dir = std::env::temp_dir().join(format!(
        "ssherlock-authorized-keys-{}-{}",
        std::process::id(),
        methods.len()
    ));
    std::fs::create_dir_all(&dir).unwrap();
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
    let mut session = Session::connect(config, host_keys, tx).await?;
    session.authenticate(methods).await?;
    Ok(session)
}

#[tokio::test]
async fn a_generated_key_is_added_once_and_then_logs_in() {
    let Some(server) = TestServer::start("a_generated_key_is_added_once_and_then_logs_in") else {
        return;
    };
    let key = keys::generate_blocking(KeyKind::Ed25519, "pushed by a test", None).unwrap();
    let stored = AuthMethod::StoredKey(StoredKey {
        label: "new".into(),
        fingerprint: key.fingerprint.clone(),
        private_key: key.private_key.clone(),
        passphrase: None,
    });
    // Not authorized yet.
    assert!(matches!(
        session(&server, std::slice::from_ref(&stored)).await,
        Err(SshError::AuthFailed { .. })
    ));

    let password = AuthMethod::SavedPassword(SecretString::from(harness::PASSWORD));
    let logged_in = session(&server, &[password]).await.unwrap();
    assert_eq!(
        logged_in.install_public_key(&key.public_key).await.unwrap(),
        KeyInstall::Added
    );
    assert_eq!(
        logged_in.install_public_key(&key.public_key).await.unwrap(),
        KeyInstall::AlreadyPresent
    );
    assert!(matches!(
        logged_in.install_public_key("ssh-ed25519 not-base64").await,
        Err(SshError::KeyUnreadable { .. })
    ));
    drop(logged_in);

    // The server now takes the key, and the fixture keys already there still work.
    assert!(session(&server, &[stored]).await.is_ok());
    let fixture = AuthMethod::KeyFile(harness::fixture("id_ed25519"));
    assert!(session(&server, &[fixture]).await.is_ok());
}

#[tokio::test]
async fn a_running_shell_adds_a_key_over_its_own_connection() {
    let Some(server) = TestServer::start("a_running_shell_adds_a_key_over_its_own_connection")
    else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("ssherlock-shell-key-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let SessionEvent::HostKey(prompt) = event {
                prompt.answer(HostKeyDecision::Trust);
            }
        }
    });
    let connect = ConnectConfig::new("127.0.0.1", harness::USER).with_port(server.addr().port());
    let config = RemoteShellConfig::new(
        connect,
        vec![AuthMethod::KeyFile(harness::fixture("id_ed25519"))],
    );
    let (io, shell) = spawn_shell(
        config,
        host_keys,
        TerminalSize::new(80, 24),
        tx,
        &tokio::runtime::Handle::current(),
    );

    // Asked straight away: the request waits for the shell to be connected.
    let key = keys::generate_blocking(KeyKind::Ed25519, "from a tab", None).unwrap();
    let added = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        shell.install_public_key(&key.public_key),
    )
    .await
    .unwrap();
    assert_eq!(added.unwrap(), KeyInstall::Added);

    let stored = AuthMethod::StoredKey(StoredKey {
        label: "from a tab".into(),
        fingerprint: key.fingerprint.clone(),
        private_key: key.private_key.clone(),
        passphrase: None,
    });
    assert!(session(&server, &[stored]).await.is_ok());

    // Once the tab is gone, so is the connection, and requests say so. The task may serve one
    // more request before it notices the terminal closed, so ask until it has.
    drop(io);
    let gone = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match shell.install_public_key(&key.public_key).await {
                Err(error) => break error,
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(gone, SshError::NotConnected), "{gone:?}");
}
