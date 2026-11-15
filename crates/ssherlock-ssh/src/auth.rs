//! Logging in: SSH agent, key files, password and keyboard-interactive, tried in order.
//!
//! **Service** layer. Each method is one small function with the same shape: it gets an
//! [`Authenticator`] and returns a [`Step`]. Anything the user must type (a password, a key
//! passphrase, a 2FA code) goes up as a [`SessionEvent::Prompt`] carrying a `oneshot` reply;
//! answers come back as [`SecretString`]s and are exposed only at the moment they're sent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey};
use russh::{MethodKind, MethodSet};
use ssherlock_core::{ExposeSecret, SecretString};
use tokio::sync::{mpsc, oneshot};

use crate::SshError;
use crate::session::{ClientHandler, SessionEvent};

/// How many times the user is asked for a password or passphrase before moving on, as OpenSSH's
/// `NumberOfPasswordPrompts`.
const MAX_PROMPTS: usize = 3;
/// Keyboard-interactive rounds within one attempt before giving up on a server that keeps asking.
const MAX_INFO_ROUNDS: usize = 16;

/// One way of logging in. A session tries a list of these in order (see [`AuthMethod::defaults`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// Keys held by an SSH agent.
    Agent(AgentSocket),
    /// A private key file, decrypted with a passphrase the user is asked for if it's encrypted.
    KeyFile(PathBuf),
    /// A password the user is asked for.
    Password,
    /// Server-driven prompts: 2FA codes, PAM conversations, a password asked the PAM way.
    KeyboardInteractive,
}

/// Where to find the SSH agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSocket {
    /// `$SSH_AUTH_SOCK`, as every SSH client does.
    FromEnvironment,
    /// A specific Unix socket.
    Path(PathBuf),
}

impl AuthMethod {
    /// OpenSSH's order: agent, then `key_files`, then password, then keyboard-interactive.
    pub fn defaults(key_files: impl IntoIterator<Item = PathBuf>) -> Vec<Self> {
        std::iter::once(Self::Agent(AgentSocket::FromEnvironment))
            .chain(key_files.into_iter().map(Self::KeyFile))
            .chain([Self::Password, Self::KeyboardInteractive])
            .collect()
    }

    fn kind(&self) -> MethodKind {
        match self {
            Self::Agent(_) | Self::KeyFile(_) => MethodKind::PublicKey,
            Self::Password => MethodKind::Password,
            Self::KeyboardInteractive => MethodKind::KeyboardInteractive,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Agent(_) => "agent",
            Self::KeyFile(_) => "key file",
            Self::Password => "password",
            Self::KeyboardInteractive => "keyboard-interactive",
        }
    }
}

/// The private key files OpenSSH tries by default, in its order, inside `ssh_dir` (usually
/// `~/.ssh`). Files that don't exist are skipped when logging in.
pub fn default_key_files(ssh_dir: &Path) -> Vec<PathBuf> {
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| ssh_dir.join(name))
        .collect()
}

/// A question for the user while logging in. Answer with [`AuthPrompt::answer`], one answer per
/// field in order; dropping it unanswered cancels the login.
#[derive(Debug)]
pub struct AuthPrompt {
    kind: AuthPromptKind,
    fields: Vec<PromptField>,
    reply: oneshot::Sender<Vec<SecretString>>,
}

/// What a prompt is for, so the UI can title it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthPromptKind {
    /// The account password.
    Password {
        /// Who is logging in.
        user: String,
        /// Where.
        host: String,
        /// The previous password was refused.
        is_retry: bool,
    },
    /// The passphrase of an encrypted private key.
    Passphrase {
        /// The key file.
        key: PathBuf,
        /// The previous passphrase was wrong.
        is_retry: bool,
    },
    /// Prompts chosen by the server (keyboard-interactive).
    Server {
        /// The server's title for the prompts, often empty.
        name: String,
        /// The server's instructions, often empty.
        instructions: String,
    },
}

/// One input in a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptField {
    /// The label, e.g. `Password:` or `Verification code:`.
    pub label: String,
    /// Whether typed text may be shown; `false` for secrets.
    pub echo: bool,
}

impl AuthPrompt {
    /// What the prompt is for.
    pub fn kind(&self) -> &AuthPromptKind {
        &self.kind
    }

    /// The inputs to show, in order.
    pub fn fields(&self) -> &[PromptField] {
        &self.fields
    }

    /// Sends the user's answers, one per field.
    pub fn answer(self, answers: Vec<SecretString>) {
        if self.reply.send(answers).is_err() {
            // The login gave up (cancelled, or the server closed the connection), so there's no
            // one to receive the answers; they're dropped and zeroed.
            tracing::warn!("login prompt answered after the login ended");
        }
    }
}

/// What one method achieved.
enum Step {
    /// Logged in.
    Success,
    /// Not logged in yet; the server now allows these methods (after a refusal, or a partial
    /// success when it wants a second factor).
    Continue(MethodSet),
    /// The method couldn't be tried (no agent, no such key file, unreadable key).
    Skipped,
}

/// State shared by the methods during one login.
pub(crate) struct Authenticator<'a> {
    pub(crate) handle: &'a mut client::Handle<ClientHandler>,
    pub(crate) user: &'a str,
    pub(crate) host: &'a str,
    pub(crate) events: &'a mpsc::Sender<SessionEvent>,
}

impl Authenticator<'_> {
    /// Tries `methods` in order until one logs in.
    pub(crate) async fn run(&mut self, methods: &[AuthMethod]) -> Result<(), SshError> {
        // "none" first, as OpenSSH does: it tells us which methods the server accepts.
        let mut allowed = match self.handle.authenticate_none(self.user).await? {
            AuthResult::Success => return Ok(()),
            AuthResult::Failure {
                remaining_methods, ..
            } => remaining_methods,
        };
        let mut tried = Vec::new();
        for method in methods {
            if !allowed.contains(&method.kind()) {
                continue;
            }
            let step = match method {
                AuthMethod::Agent(socket) => self.agent(socket).await?,
                AuthMethod::KeyFile(path) => self.key_file(path).await?,
                AuthMethod::Password => self.password().await?,
                AuthMethod::KeyboardInteractive => self.keyboard_interactive().await?,
            };
            match step {
                Step::Success => {
                    tracing::info!(method = method.label(), "logged in");
                    return Ok(());
                }
                Step::Continue(remaining) => {
                    tried.push(method.label());
                    allowed = remaining;
                }
                Step::Skipped => {}
            }
        }
        tried.dedup();
        Err(SshError::AuthFailed {
            user: self.user.to_owned(),
            tried,
        })
    }

    async fn agent(&mut self, socket: &AgentSocket) -> Result<Step, SshError> {
        let mut agent = match connect_agent(socket).await {
            Ok(agent) => agent,
            Err(e) => {
                // No agent is normal; the next method gets its turn.
                tracing::debug!(error = %e, "no SSH agent to log in with");
                return Ok(Step::Skipped);
            }
        };
        if let Some(binding) = self.handle.agent_session_binding()
            && let Err(e) = agent.bind_session(binding, false).await
        {
            // Older agents don't know session binding; plain keys still work without it.
            tracing::debug!(error = %e, "SSH agent did not accept session binding");
        }
        let identities = match agent.request_identities().await {
            Ok(identities) => identities,
            Err(e) => {
                tracing::warn!(error = %e, "could not list the SSH agent's keys; skipping the agent");
                return Ok(Step::Skipped);
            }
        };
        let mut remaining = None;
        for identity in identities {
            let key = identity.public_key().into_owned();
            let hash = self.rsa_hash(&key).await?;
            match self
                .handle
                .authenticate_publickey_with(self.user, key, hash, &mut agent)
                .await
            {
                Ok(AuthResult::Success) => return Ok(Step::Success),
                Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                }) => {
                    let done =
                        partial_success || !remaining_methods.contains(&MethodKind::PublicKey);
                    remaining = Some(remaining_methods);
                    if done {
                        break;
                    }
                }
                Err(e) => {
                    // The agent declined to sign this key (locked, or needs a confirmation the
                    // user refused); its other keys may still work.
                    tracing::warn!(error = %e, "SSH agent did not sign; trying its next key");
                }
            }
        }
        Ok(remaining.map_or(Step::Skipped, Step::Continue))
    }

    async fn key_file(&mut self, path: &Path) -> Result<Step, SshError> {
        let mut passphrase: Option<SecretString> = None;
        let mut asked = 0;
        let key = loop {
            match load_key(path, passphrase.clone()).await {
                Ok(key) => break key,
                Err(KeyLoad::Missing) => {
                    tracing::debug!(path = %path.display(), "no key file here");
                    return Ok(Step::Skipped);
                }
                Err(KeyLoad::NeedsPassphrase) if asked < MAX_PROMPTS => {
                    let kind = AuthPromptKind::Passphrase {
                        key: path.to_owned(),
                        is_retry: asked > 0,
                    };
                    passphrase = Some(self.ask_one(kind, "Passphrase:").await?);
                    asked += 1;
                }
                Err(KeyLoad::NeedsPassphrase) => {
                    tracing::warn!(path = %path.display(), "wrong passphrase {MAX_PROMPTS} times; skipping this key");
                    return Ok(Step::Skipped);
                }
                Err(KeyLoad::Unusable(reason)) => {
                    tracing::warn!(path = %path.display(), %reason, "could not read the key file; skipping it");
                    return Ok(Step::Skipped);
                }
            }
        };
        let hash = self.rsa_hash(key.public_key()).await?;
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        Ok(step_from(
            self.handle.authenticate_publickey(self.user, key).await?,
        ))
    }

    async fn password(&mut self) -> Result<Step, SshError> {
        let mut last = None;
        for attempt in 0..MAX_PROMPTS {
            let kind = AuthPromptKind::Password {
                user: self.user.to_owned(),
                host: self.host.to_owned(),
                is_retry: attempt > 0,
            };
            let password = self.ask_one(kind, "Password:").await?;
            // russh takes an owned String for the wire; this is the one place it's exposed.
            let result = self
                .handle
                .authenticate_password(self.user, password.expose_secret())
                .await?;
            match step_from(result) {
                Step::Continue(remaining) if remaining.contains(&MethodKind::Password) => {
                    last = Some(remaining);
                }
                step => return Ok(step),
            }
        }
        Ok(last.map_or(Step::Skipped, Step::Continue))
    }

    async fn keyboard_interactive(&mut self) -> Result<Step, SshError> {
        let mut last = None;
        for _attempt in 0..MAX_PROMPTS {
            let mut response = self
                .handle
                .authenticate_keyboard_interactive_start(self.user, None)
                .await?;
            let mut rounds = 0;
            let remaining = loop {
                match response {
                    KeyboardInteractiveAuthResponse::Success => return Ok(Step::Success),
                    KeyboardInteractiveAuthResponse::Failure {
                        remaining_methods,
                        partial_success,
                    } => {
                        if partial_success {
                            return Ok(Step::Continue(remaining_methods));
                        }
                        break remaining_methods;
                    }
                    KeyboardInteractiveAuthResponse::InfoRequest {
                        name,
                        instructions,
                        prompts,
                    } => {
                        rounds += 1;
                        if rounds > MAX_INFO_ROUNDS {
                            return Err(SshError::TooManyPrompts);
                        }
                        let answers = if prompts.is_empty() {
                            Vec::new()
                        } else {
                            let fields = prompts
                                .into_iter()
                                .map(|p| PromptField {
                                    label: p.prompt,
                                    echo: p.echo,
                                })
                                .collect();
                            self.ask(AuthPromptKind::Server { name, instructions }, fields)
                                .await?
                        };
                        // As with passwords: russh needs owned Strings, exposed only here.
                        let answers = answers
                            .iter()
                            .map(|a| a.expose_secret().to_owned())
                            .collect();
                        response = self
                            .handle
                            .authenticate_keyboard_interactive_respond(answers)
                            .await?;
                    }
                }
            };
            if !remaining.contains(&MethodKind::KeyboardInteractive) {
                return Ok(Step::Continue(remaining));
            }
            last = Some(remaining);
        }
        Ok(last.map_or(Step::Skipped, Step::Continue))
    }

    /// The signature hash to use with `key`: SHA-2 for RSA unless the server only knows SHA-1;
    /// `None` (meaning "not applicable") for other key types.
    async fn rsa_hash(&self, key: &PublicKey) -> Result<Option<HashAlg>, SshError> {
        if !key.algorithm().is_rsa() {
            return Ok(None);
        }
        // `None` from russh means the server didn't list its signature algorithms; every
        // OpenSSH since 7.2 accepts SHA-512, so assume it rather than fall back to SHA-1.
        Ok(self
            .handle
            .best_supported_rsa_hash()
            .await?
            .unwrap_or(Some(HashAlg::Sha512)))
    }

    async fn ask_one(&self, kind: AuthPromptKind, label: &str) -> Result<SecretString, SshError> {
        let field = PromptField {
            label: label.to_owned(),
            echo: false,
        };
        self.ask(kind, vec![field])
            .await?
            .into_iter()
            .next()
            .ok_or(SshError::AuthCancelled)
    }

    async fn ask(
        &self,
        kind: AuthPromptKind,
        fields: Vec<PromptField>,
    ) -> Result<Vec<SecretString>, SshError> {
        let expected = fields.len();
        let (reply, answer) = oneshot::channel();
        let prompt = AuthPrompt {
            kind,
            fields,
            reply,
        };
        self.events
            .send(SessionEvent::Prompt(prompt))
            .await
            .map_err(|_| SshError::AuthCancelled)?;
        let answers = answer.await.map_err(|_| SshError::AuthCancelled)?;
        if answers.len() == expected {
            Ok(answers)
        } else {
            Err(SshError::PromptAnswerCount {
                expected,
                got: answers.len(),
            })
        }
    }
}

fn step_from(result: AuthResult) -> Step {
    match result {
        AuthResult::Success => Step::Success,
        AuthResult::Failure {
            remaining_methods, ..
        } => Step::Continue(remaining_methods),
    }
}

type DynAgent =
    AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin + 'static>>;

#[cfg(unix)]
async fn connect_agent(socket: &AgentSocket) -> Result<DynAgent, russh::keys::Error> {
    let agent = match socket {
        AgentSocket::FromEnvironment => AgentClient::connect_env().await?,
        AgentSocket::Path(path) => AgentClient::connect_uds(path).await?,
    };
    Ok(agent.dynamic())
}

#[cfg(not(unix))]
async fn connect_agent(_socket: &AgentSocket) -> Result<DynAgent, russh::keys::Error> {
    // Windows agents (Pageant, OpenSSH's named pipe) come with Windows support, if it's added.
    Err(russh::keys::Error::AgentFailure)
}

/// Why a key file couldn't be loaded.
enum KeyLoad {
    Missing,
    /// Encrypted and no passphrase was given, or the given one was wrong.
    NeedsPassphrase,
    Unusable(String),
}

/// Reads and decrypts a key file off the async runtime: the bcrypt KDF of an encrypted OpenSSH
/// key deliberately takes a noticeable fraction of a second.
async fn load_key(
    path: &Path,
    passphrase: Option<SecretString>,
) -> Result<russh::keys::PrivateKey, KeyLoad> {
    let path = path.to_owned();
    let had_passphrase = passphrase.is_some();
    let loaded = tokio::task::spawn_blocking(move || {
        russh::keys::load_secret_key(&path, passphrase.as_ref().map(ExposeSecret::expose_secret))
    })
    .await;
    match loaded {
        Ok(Ok(key)) => Ok(key),
        Ok(Err(russh::keys::Error::IO(e))) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(KeyLoad::Missing)
        }
        Ok(Err(russh::keys::Error::KeyIsEncrypted)) => Err(KeyLoad::NeedsPassphrase),
        // With a passphrase, a decoding failure almost always means the passphrase was wrong.
        Ok(Err(_)) if had_passphrase => Err(KeyLoad::NeedsPassphrase),
        Ok(Err(e)) => Err(KeyLoad::Unusable(e.to_string())),
        Err(e) => Err(KeyLoad::Unusable(format!("key loading task failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn default_order_matches_openssh() {
        let keys = default_key_files(Path::new("/home/u/.ssh"));
        assert_eq!(
            keys,
            vec![
                PathBuf::from("/home/u/.ssh/id_ed25519"),
                PathBuf::from("/home/u/.ssh/id_ecdsa"),
                PathBuf::from("/home/u/.ssh/id_rsa"),
            ]
        );
        let methods = AuthMethod::defaults(keys.into_iter().take(1));
        assert_eq!(
            methods,
            vec![
                AuthMethod::Agent(AgentSocket::FromEnvironment),
                AuthMethod::KeyFile(PathBuf::from("/home/u/.ssh/id_ed25519")),
                AuthMethod::Password,
                AuthMethod::KeyboardInteractive,
            ]
        );
    }

    #[tokio::test]
    async fn key_loading_outcomes() {
        assert!(load_key(&fixture("id_ed25519"), None).await.is_ok());
        assert!(load_key(&fixture("id_rsa"), None).await.is_ok());
        assert!(matches!(
            load_key(&fixture("does-not-exist"), None).await,
            Err(KeyLoad::Missing)
        ));
        let encrypted = fixture("id_ed25519_encrypted");
        assert!(matches!(
            load_key(&encrypted, None).await,
            Err(KeyLoad::NeedsPassphrase)
        ));
        let wrong = Some(SecretString::from("wrong"));
        assert!(matches!(
            load_key(&encrypted, wrong).await,
            Err(KeyLoad::NeedsPassphrase)
        ));
        let right = Some(SecretString::from("fixture-passphrase"));
        assert!(load_key(&encrypted, right).await.is_ok());
        assert!(matches!(
            load_key(&fixture("README.md"), None).await,
            Err(KeyLoad::Unusable(_))
        ));
    }
}
