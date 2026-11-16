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
    AuthMethod, ConnectConfig, HostKeys, KeyInstall, Session, SessionEvent, SshError, StoredKey,
    keys,
};
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
