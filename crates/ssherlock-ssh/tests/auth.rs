//! Login against the Docker OpenSSH server with every method: key files (each type, encrypted),
//! a throwaway `ssh-agent`, password and keyboard-interactive (PAM).
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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use harness::{TestServer, fixture};
use ssherlock_core::SecretString;
use ssherlock_ssh::auth::AuthPromptKind;
use ssherlock_ssh::known_hosts::HostHashing;
use ssherlock_ssh::session::HostKeyDecision;
use ssherlock_ssh::{
    AgentSocket, AuthMethod, ConnectConfig, HostKeys, Session, SessionEvent, SshError,
};
use tokio::sync::mpsc;

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssherlock-auth-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// What the fake user does with each login prompt, in order.
#[derive(Clone)]
enum Reply {
    Answer(&'static str),
    Dismiss,
}

/// Trusts the host key, answers login prompts from `replies`, and returns the prompts' kinds.
fn fake_user(
    mut events: mpsc::Receiver<SessionEvent>,
    replies: Vec<Reply>,
) -> tokio::task::JoinHandle<Vec<AuthPromptKind>> {
    tokio::spawn(async move {
        let mut replies = replies.into_iter();
        let mut seen = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::HostKey(prompt) => prompt.answer(HostKeyDecision::Trust),
                SessionEvent::Prompt(prompt) => {
                    seen.push(prompt.kind().clone());
                    match replies.next() {
                        Some(Reply::Answer(text)) => {
                            let answers = prompt
                                .fields()
                                .iter()
                                .map(|_| SecretString::from(text))
                                .collect();
                            prompt.answer(answers);
                        }
                        Some(Reply::Dismiss) => drop(prompt),
                        None => panic!("unexpected prompt {:?}", prompt.kind()),
                    }
                }
                SessionEvent::Banner(_) => {}
            }
        }
        seen
    })
}

/// Connects and logs in with `methods`; returns the result and the prompts the user saw.
async fn login(
    server: &TestServer,
    test: &str,
    methods: &[AuthMethod],
    replies: Vec<Reply>,
) -> (Result<(), SshError>, Vec<AuthPromptKind>) {
    let dir = temp_dir(test);
    let host_keys = HostKeys::new(dir.join("known_hosts"), vec![], HostHashing::Plain);
    let (tx, rx) = mpsc::channel(4);
    let user = fake_user(rx, replies);
    let config = ConnectConfig::new("127.0.0.1", harness::USER).with_port(server.addr().port());
    let mut session = Session::connect(config, host_keys, tx).await.unwrap();
    let result = session.authenticate(methods).await;
    drop(session);
    (result, user.await.unwrap())
}

fn password_prompt(is_retry: bool) -> AuthPromptKind {
    AuthPromptKind::Password {
        user: harness::USER.to_owned(),
        host: "127.0.0.1".to_owned(),
        is_retry,
    }
}

#[tokio::test]
async fn key_files_of_every_type() {
    let Some(server) = TestServer::start("key_files_of_every_type") else {
        return;
    };
    for key in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let (result, prompts) =
            login(&server, key, &[AuthMethod::KeyFile(fixture(key))], vec![]).await;
        assert!(result.is_ok(), "{key}: {result:?}");
        assert_eq!(prompts, vec![], "{key}");
    }
}

#[tokio::test]
async fn encrypted_key_asks_for_its_passphrase_until_right() {
    let Some(server) = TestServer::start("encrypted_key_asks_for_its_passphrase_until_right")
    else {
        return;
    };
    let key = fixture("id_ed25519_encrypted");
    let replies = vec![
        Reply::Answer("wrong"),
        Reply::Answer(harness::KEY_PASSPHRASE),
    ];
    let (result, prompts) = login(
        &server,
        "encrypted",
        &[AuthMethod::KeyFile(key.clone())],
        replies,
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        prompts,
        vec![
            AuthPromptKind::Passphrase {
                key: key.clone(),
                is_retry: false
            },
            AuthPromptKind::Passphrase {
                key,
                is_retry: true
            },
        ]
    );
}

#[tokio::test]
async fn refused_key_then_missing_key_then_password() {
    let Some(server) = TestServer::start("refused_key_then_missing_key_then_password") else {
        return;
    };
    let methods = [
        AuthMethod::KeyFile(fixture("id_unauthorized")),
        AuthMethod::KeyFile(fixture("does-not-exist")),
        AuthMethod::Password,
    ];
    let replies = vec![
        Reply::Answer("not-the-password"),
        Reply::Answer(harness::PASSWORD),
    ];
    let (result, prompts) = login(&server, "fallback", &methods, replies).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(prompts, vec![password_prompt(false), password_prompt(true)]);
}

#[tokio::test]
async fn every_method_refused_is_auth_failed() {
    let Some(server) = TestServer::start("every_method_refused_is_auth_failed") else {
        return;
    };
    let methods = [
        AuthMethod::KeyFile(fixture("id_unauthorized")),
        AuthMethod::Password,
    ];
    let replies = vec![Reply::Answer("nope"); 3];
    let (result, prompts) = login(&server, "refused", &methods, replies).await;
    match result {
        Err(SshError::AuthFailed { user, tried }) => {
            assert_eq!(user, harness::USER);
            assert_eq!(tried, vec!["key file", "password"]);
        }
        other => panic!("expected AuthFailed, got {other:?}"),
    }
    assert_eq!(prompts.len(), 3, "asked three times, like OpenSSH");
}

#[tokio::test]
async fn keyboard_interactive_through_pam() {
    let Some(server) = TestServer::start("keyboard_interactive_through_pam") else {
        return;
    };
    let (result, prompts) = login(
        &server,
        "kbdint",
        &[AuthMethod::KeyboardInteractive],
        vec![Reply::Answer(harness::PASSWORD)],
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(
        matches!(prompts.as_slice(), [AuthPromptKind::Server { .. }]),
        "{prompts:?}"
    );
}

#[tokio::test]
async fn dismissing_a_prompt_cancels_the_login() {
    let Some(server) = TestServer::start("dismissing_a_prompt_cancels_the_login") else {
        return;
    };
    let methods = [
        AuthMethod::Password,
        AuthMethod::KeyFile(fixture("id_ed25519")),
    ];
    let (result, _) = login(&server, "dismiss", &methods, vec![Reply::Dismiss]).await;
    assert!(matches!(result, Err(SshError::AuthCancelled)), "{result:?}");
}

/// An `ssh-agent` started for one test, on a socket in that test's directory. Killed on drop.
struct TestAgent {
    child: Child,
    socket: PathBuf,
}

impl TestAgent {
    /// Starts an agent holding `key`, or `None` if `ssh-agent`/`ssh-add` aren't installed.
    fn start(dir: &Path, key: &Path) -> Option<Self> {
        let socket = dir.join("agent.sock");
        let child = Command::new("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdout(Stdio::null())
            .spawn()
            .ok()?;
        let agent = Self { child, socket };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !agent.socket.exists() {
            assert!(
                Instant::now() < deadline,
                "ssh-agent did not create its socket"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // ssh-add refuses keys readable by others; git checks fixtures out as 0644.
        let private = dir.join("agent_key");
        std::fs::copy(key, &private).unwrap();
        restrict(&private);
        let added = Command::new("ssh-add")
            .arg(&private)
            .env("SSH_AUTH_SOCK", &agent.socket)
            .stderr(Stdio::null())
            .status()
            .ok()?;
        assert!(added.success(), "ssh-add failed");
        Some(agent)
    }
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

impl Drop for TestAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn agent_signs_for_us() {
    let Some(server) = TestServer::start("agent_signs_for_us") else {
        return;
    };
    let dir = temp_dir("agent-socket");
    let Some(agent) = TestAgent::start(&dir, &fixture("id_ecdsa")) else {
        #[allow(clippy::print_stderr, reason = "explain the skip")]
        {
            eprintln!("skipping agent_signs_for_us: ssh-agent or ssh-add is not installed");
        }
        return;
    };
    let methods = [
        AuthMethod::Agent(AgentSocket::Path(agent.socket.clone())),
        AuthMethod::Password,
    ];
    let (result, prompts) = login(&server, "agent", &methods, vec![]).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(prompts, vec![], "the agent's key should be enough");
}

#[tokio::test]
async fn missing_agent_is_skipped() {
    let Some(server) = TestServer::start("missing_agent_is_skipped") else {
        return;
    };
    let methods = [
        AuthMethod::Agent(AgentSocket::Path(PathBuf::from("/nonexistent/agent.sock"))),
        AuthMethod::KeyFile(fixture("id_ed25519")),
    ];
    let (result, prompts) = login(&server, "no-agent", &methods, vec![]).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(prompts, vec![]);
}
