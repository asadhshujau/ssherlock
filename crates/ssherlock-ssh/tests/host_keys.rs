//! Host key verification against the Docker OpenSSH server: trust on first use, a changed key
//! refused, a revoked key refused without asking.
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

use std::path::PathBuf;

use harness::{TestServer, fixture};
use russh::keys::PublicKey;
use ssherlock_ssh::known_hosts::{HostHashing, HostKeyStatus, KnownHostName, KnownHosts};
use ssherlock_ssh::session::{HostKeyConcern, HostKeyDecision};
use ssherlock_ssh::{ConnectConfig, HostKeys, Session, SessionEvent, SshError};
use tokio::sync::mpsc;

/// A fresh directory for one test's `known_hosts`.
fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssherlock-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(server: &TestServer) -> ConnectConfig {
    ConnectConfig::new("127.0.0.1", harness::USER).with_port(server.addr().port())
}

fn name(server: &TestServer) -> KnownHostName {
    KnownHostName::new("127.0.0.1", server.addr().port())
}

/// Answers every host key question with `decision` and returns how many were asked.
fn answer_all(
    mut events: mpsc::Receiver<SessionEvent>,
    decision: HostKeyDecision,
) -> tokio::task::JoinHandle<Vec<HostKeyConcern>> {
    tokio::spawn(async move {
        let mut asked = Vec::new();
        while let Some(event) = events.recv().await {
            if let SessionEvent::HostKey(prompt) = event {
                assert!(
                    prompt.fingerprint().starts_with("SHA256:"),
                    "{}",
                    prompt.fingerprint()
                );
                assert!(
                    prompt.randomart().contains("[SHA256]"),
                    "{}",
                    prompt.randomart()
                );
                asked.push(prompt.concern().clone());
                prompt.answer(decision);
            }
        }
        asked
    })
}

#[tokio::test]
async fn first_connection_asks_once_then_remembers() {
    let Some(server) = TestServer::start("first_connection_asks_once_then_remembers") else {
        return;
    };
    let dir = temp_dir("tofu");
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Hashed);

    let (tx, rx) = mpsc::channel(4);
    let answers = answer_all(rx, HostKeyDecision::Trust);
    let session = Session::connect(config(&server), host_keys.clone(), tx)
        .await
        .unwrap();
    session.disconnect().await.unwrap();
    drop(session);
    let asked = answers.await.unwrap();
    assert_eq!(
        asked,
        vec![HostKeyConcern::New {
            other_algorithms: vec![]
        }]
    );

    // Saved: the file now trusts exactly the server's Ed25519 host key.
    let server_key = PublicKey::from_openssh(&server.host_key("ed25519")).unwrap();
    let known = KnownHosts::load(dir.join("known_hosts")).await.unwrap();
    assert_eq!(
        known.check(&name(&server), &server_key),
        HostKeyStatus::Trusted
    );

    // Second time: no question.
    let (tx, rx) = mpsc::channel(4);
    let answers = answer_all(rx, HostKeyDecision::Reject);
    let session = Session::connect(config(&server), host_keys, tx)
        .await
        .unwrap();
    drop(session);
    assert_eq!(answers.await.unwrap(), vec![]);
}

#[tokio::test]
async fn changed_key_is_refused_when_the_user_says_no() {
    let Some(server) = TestServer::start("changed_key_is_refused_when_the_user_says_no") else {
        return;
    };
    let dir = temp_dir("changed");
    let path = dir.join("known_hosts");
    let impostor = PublicKey::read_openssh_file(fixture("id_unauthorized.pub")).unwrap();
    let mut known = KnownHosts::empty(&path);
    known
        .add(&name(&server), &impostor, HostHashing::Plain)
        .unwrap();
    known.save().await.unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let (tx, rx) = mpsc::channel(4);
    let answers = answer_all(rx, HostKeyDecision::Reject);
    let result = Session::connect(
        config(&server),
        HostKeys::new(&path, vec![], HostHashing::Plain),
        tx,
    )
    .await;
    assert!(
        matches!(result, Err(SshError::HostKeyRejected { .. })),
        "{result:?}"
    );
    let asked = answers.await.unwrap();
    assert!(
        matches!(asked.as_slice(), [HostKeyConcern::Changed { known }] if known.len() == 1),
        "{asked:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "a rejected key must not be saved"
    );
}

#[tokio::test]
async fn changed_key_replaces_the_old_one_when_trusted() {
    let Some(server) = TestServer::start("changed_key_replaces_the_old_one_when_trusted") else {
        return;
    };
    let dir = temp_dir("replaced");
    let path = dir.join("known_hosts");
    let impostor = PublicKey::read_openssh_file(fixture("id_unauthorized.pub")).unwrap();
    let mut known = KnownHosts::empty(&path);
    known
        .add(&name(&server), &impostor, HostHashing::Plain)
        .unwrap();
    known.save().await.unwrap();

    let (tx, rx) = mpsc::channel(4);
    let answers = answer_all(rx, HostKeyDecision::Trust);
    let session = Session::connect(
        config(&server),
        HostKeys::new(&path, vec![], HostHashing::Plain),
        tx,
    )
    .await;
    drop(session.unwrap());
    assert_eq!(answers.await.unwrap().len(), 1);

    let known = KnownHosts::load(&path).await.unwrap();
    assert_eq!(
        known.to_text().lines().count(),
        1,
        "old key replaced, not kept: {}",
        known.to_text()
    );
    let server_key = PublicKey::from_openssh(&server.host_key("ed25519")).unwrap();
    assert_eq!(
        known.check(&name(&server), &server_key),
        HostKeyStatus::Trusted
    );
}

#[tokio::test]
async fn revoked_key_is_refused_without_asking() {
    let Some(server) = TestServer::start("revoked_key_is_refused_without_asking") else {
        return;
    };
    let dir = temp_dir("revoked");
    let path = dir.join("known_hosts");
    let server_key = server.host_key("ed25519");
    std::fs::write(&path, format!("@revoked * {server_key}\n")).unwrap();

    let (tx, rx) = mpsc::channel(4);
    let answers = answer_all(rx, HostKeyDecision::Trust);
    let result = Session::connect(
        config(&server),
        HostKeys::new(&path, vec![], HostHashing::Plain),
        tx,
    )
    .await;
    assert!(
        matches!(result, Err(SshError::HostKeyRevoked { line: 1, .. })),
        "{result:?}"
    );
    assert_eq!(answers.await.unwrap(), vec![]);
}

#[tokio::test]
async fn unanswered_question_rejects() {
    let Some(server) = TestServer::start("unanswered_question_rejects") else {
        return;
    };
    let dir = temp_dir("unanswered");
    let (tx, mut rx) = mpsc::channel(4);
    // Drop each prompt without answering, as a closed dialog would.
    let listener = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = Session::connect(
        config(&server),
        HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain),
        tx,
    )
    .await;
    assert!(
        matches!(result, Err(SshError::HostKeyRejected { .. })),
        "{result:?}"
    );
    listener.await.unwrap();
    assert!(!dir.join("known_hosts").exists());
}
