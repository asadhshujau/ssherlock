//! Logging in: keychain keys, SSH agent, key files, saved and typed passwords, and
//! keyboard-interactive, tried in order.
//!
//! **Service** layer. Each method is one small function with the same shape: it gets an
//! [`Authenticator`] and returns a [`Step`]. Anything the user must type (a password, a key
//! passphrase, a 2FA code) goes up as a [`SessionEvent::Prompt`] carrying a `oneshot` reply;
//! answers come back as [`SecretString`]s and are exposed only at the moment they're sent.
//! Typed passphrases are remembered for a while in a [`PassphraseCache`] (KEY-11).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// How long a typed passphrase is remembered by default (KEY-11).
pub const DEFAULT_PASSPHRASE_TTL: Duration = Duration::from_secs(15 * 60);

/// One way of logging in. A session tries a list of these in order (see [`AuthMethod::defaults`]).
#[derive(Clone, Debug)]
pub enum AuthMethod {
    /// A key from the keychain, already opened from the vault.
    StoredKey(StoredKey),
    /// Keys held by an SSH agent.
    Agent(AgentSocket),
    /// A private key file, decrypted with a passphrase the user is asked for if it's encrypted.
    KeyFile(PathBuf),
    /// A saved password, tried once; if it's refused, [`AuthMethod::Password`] asks.
    SavedPassword(SecretString),
    /// A password the user is asked for.
    Password,
    /// Server-driven prompts: 2FA codes, PAM conversations, a password asked the PAM way.
    KeyboardInteractive,
}

/// A keychain key to log in with.
#[derive(Clone)]
pub struct StoredKey {
    /// The name shown when asking for its passphrase.
    pub label: String,
    /// Its `SHA256:…` fingerprint; also what its passphrase is cached under.
    pub fingerprint: String,
    /// The private key in OpenSSH format, encrypted if it has a passphrase.
    pub private_key: SecretString,
    /// The saved passphrase, if the user saved it.
    pub passphrase: Option<SecretString>,
}

impl fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredKey")
            .field("label", &self.label)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// Passphrases the user typed, remembered for a while so the next connection with the same key
/// doesn't ask again (KEY-11). Clones share one cache.
///
/// Entries expire `ttl` after they were typed; an expired one is wiped by a timer on the Tokio
/// runtime, or on the next access when there is none.
#[derive(Clone)]
pub struct PassphraseCache {
    entries: Arc<Mutex<HashMap<String, Cached>>>,
    ttl: Duration,
}

struct Cached {
    passphrase: SecretString,
    expires: Instant,
}

impl fmt::Debug for PassphraseCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PassphraseCache")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl Default for PassphraseCache {
    fn default() -> Self {
        Self::new(DEFAULT_PASSPHRASE_TTL)
    }
}

impl PassphraseCache {
    /// An empty cache whose entries last `ttl`. A zero `ttl` remembers nothing.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::default(),
            ttl,
        }
    }

    /// The passphrase remembered for `key` (a fingerprint or key file path), if it hasn't
    /// expired.
    pub fn get(&self, key: &str) -> Option<SecretString> {
        let mut entries = self.lock()?;
        purge(&mut entries);
        entries.get(key).map(|cached| cached.passphrase.clone())
    }

    /// Remembers `passphrase` for `key`.
    pub fn insert(&self, key: &str, passphrase: SecretString) {
        if self.ttl.is_zero() {
            return;
        }
        let Some(mut entries) = self.lock() else {
            return;
        };
        purge(&mut entries);
        entries.insert(
            key.to_owned(),
            Cached {
                passphrase,
                expires: Instant::now() + self.ttl,
            },
        );
        drop(entries);
        // Wipe it on time even if nothing asks again.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let entries = Arc::downgrade(&self.entries);
            let ttl = self.ttl;
            runtime.spawn(async move {
                tokio::time::sleep(ttl).await;
                if let Some(entries) = entries.upgrade()
                    && let Ok(mut entries) = entries.lock()
                {
                    purge(&mut entries);
                }
            });
        }
    }

    /// Forgets the passphrase for `key`, e.g. when it turned out to be wrong.
    pub fn forget(&self, key: &str) {
        if let Some(mut entries) = self.lock() {
            entries.remove(key);
        }
    }

    /// Forgets every passphrase.
    pub fn clear(&self) {
        if let Some(mut entries) = self.lock() {
            entries.clear();
        }
    }

    fn lock(&self) -> Option<std::sync::MutexGuard<'_, HashMap<String, Cached>>> {
        let entries = self.entries.lock().ok();
        if entries.is_none() {
            // Another thread panicked mid-update. Acting as empty only means asking again.
            tracing::warn!("the passphrase cache is unavailable after a crash; asking again");
        }
        entries
    }
}

fn purge(entries: &mut HashMap<String, Cached>) {
    let now = Instant::now();
    entries.retain(|_, cached| cached.expires > now);
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

    /// The keychain's credentials first, then [`defaults`](Self::defaults): `key`, the agent,
    /// `key_files`, `password` once, then asking for a password, then keyboard-interactive.
    pub fn with_saved(
        key: Option<StoredKey>,
        password: Option<SecretString>,
        key_files: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<Self> {
        let mut methods: Vec<Self> = key.into_iter().map(Self::StoredKey).collect();
        let mut password = password;
        for method in Self::defaults(key_files) {
            if matches!(method, Self::Password)
                && let Some(password) = password.take()
            {
                methods.push(Self::SavedPassword(password));
            }
            methods.push(method);
        }
        methods
    }

    fn kind(&self) -> MethodKind {
        match self {
            Self::StoredKey(_) | Self::Agent(_) | Self::KeyFile(_) => MethodKind::PublicKey,
            Self::SavedPassword(_) | Self::Password => MethodKind::Password,
            Self::KeyboardInteractive => MethodKind::KeyboardInteractive,
        }
    }

    /// A short name for logs and errors.
    pub fn label(&self) -> &'static str {
        match self {
            Self::StoredKey(_) => "saved key",
            Self::Agent(_) => "agent",
            Self::KeyFile(_) => "key file",
            Self::SavedPassword(_) => "saved password",
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
        /// The key: a file's path, or a keychain key's name.
        key: String,
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
    pub(crate) passphrases: Option<&'a PassphraseCache>,
    /// A saved password was refused, so the first password prompt says so.
    pub(crate) saved_password_refused: bool,
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
                AuthMethod::StoredKey(key) => self.stored_key(key).await?,
                AuthMethod::Agent(socket) => self.agent(socket).await?,
                AuthMethod::KeyFile(path) => self.key_file(path).await?,
                AuthMethod::SavedPassword(password) => self.saved_password(password).await?,
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
        let name = path.display().to_string();
        let loaded = self
            .unlock(&name, &name, None, |passphrase| {
                load_key(path.to_owned(), passphrase)
            })
            .await?;
        match loaded {
            Some(key) => self.sign_in(key).await,
            None => Ok(Step::Skipped),
        }
    }

    async fn stored_key(&mut self, key: &StoredKey) -> Result<Step, SshError> {
        let private_key = key.private_key.clone();
        let loaded = self
            .unlock(
                &key.label,
                &key.fingerprint,
                key.passphrase.clone(),
                |passphrase| open_stored(private_key.clone(), passphrase),
            )
            .await?;
        match loaded {
            Some(key) => self.sign_in(key).await,
            None => Ok(Step::Skipped),
        }
    }

    /// Loads a key with `load`, which is given a passphrase: first `saved`, else one cached
    /// under `cache_key`, else none; then, while it says one is needed, what the user types
    /// when asked about `name`. A typed passphrase that works is cached. `None` means the key
    /// can't be used and the next method gets its turn.
    async fn unlock<F, Fut>(
        &self,
        name: &str,
        cache_key: &str,
        saved: Option<SecretString>,
        load: F,
    ) -> Result<Option<russh::keys::PrivateKey>, SshError>
    where
        F: Fn(Option<SecretString>) -> Fut,
        Fut: std::future::Future<Output = Result<russh::keys::PrivateKey, KeyLoad>>,
    {
        let cached = saved
            .is_none()
            .then(|| self.passphrases.and_then(|c| c.get(cache_key)))
            .flatten();
        let mut is_cached = cached.is_some();
        let mut passphrase = saved.or(cached);
        let mut asked = 0;
        loop {
            match load(passphrase.clone()).await {
                Ok(key) => {
                    if asked > 0
                        && let (Some(cache), Some(typed)) = (self.passphrases, passphrase)
                    {
                        cache.insert(cache_key, typed);
                    }
                    return Ok(Some(key));
                }
                Err(KeyLoad::Missing) => {
                    tracing::debug!(key = name, "no key file here");
                    return Ok(None);
                }
                Err(KeyLoad::NeedsPassphrase) if asked < MAX_PROMPTS => {
                    if is_cached {
                        if let Some(cache) = self.passphrases {
                            cache.forget(cache_key);
                        }
                        is_cached = false;
                    }
                    let kind = AuthPromptKind::Passphrase {
                        key: name.to_owned(),
                        // A saved or cached passphrase that failed counts as a first try.
                        is_retry: asked > 0 || passphrase.is_some(),
                    };
                    passphrase = Some(self.ask_one(kind, "Passphrase:").await?);
                    asked += 1;
                }
                Err(KeyLoad::NeedsPassphrase) => {
                    tracing::warn!(
                        key = name,
                        "wrong passphrase {MAX_PROMPTS} times; skipping this key"
                    );
                    return Ok(None);
                }
                Err(KeyLoad::Unusable(reason)) => {
                    tracing::warn!(key = name, %reason, "could not read the key; skipping it");
                    return Ok(None);
                }
            }
        }
    }

    /// Offers `key` to the server.
    async fn sign_in(&mut self, key: russh::keys::PrivateKey) -> Result<Step, SshError> {
        let hash = self.rsa_hash(key.public_key()).await?;
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        Ok(step_from(
            self.handle.authenticate_publickey(self.user, key).await?,
        ))
    }

    async fn saved_password(&mut self, password: &SecretString) -> Result<Step, SshError> {
        // russh takes an owned String for the wire; this is the one place it's exposed.
        let result = self
            .handle
            .authenticate_password(self.user, password.expose_secret())
            .await?;
        let step = step_from(result);
        if matches!(step, Step::Continue(_)) {
            tracing::info!("the saved password was refused");
            self.saved_password_refused = true;
        }
        Ok(step)
    }

    async fn password(&mut self) -> Result<Step, SshError> {
        let mut last = None;
        for attempt in 0..MAX_PROMPTS {
            let kind = AuthPromptKind::Password {
                user: self.user.to_owned(),
                host: self.host.to_owned(),
                is_retry: attempt > 0 || self.saved_password_refused,
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

/// Decrypts a keychain key off the async runtime, as [`load_key`] does for files.
async fn open_stored(
    private_key: SecretString,
    passphrase: Option<SecretString>,
) -> Result<russh::keys::PrivateKey, KeyLoad> {
    let opened = tokio::task::spawn_blocking(move || {
        crate::keys::open_blocking(&private_key, passphrase.as_ref())
    })
    .await;
    match opened {
        Ok(Ok(key)) => Ok(key),
        Ok(Err(SshError::KeyNeedsPassphrase | SshError::KeyWrongPassphrase)) => {
            Err(KeyLoad::NeedsPassphrase)
        }
        Ok(Err(e)) => Err(KeyLoad::Unusable(e.to_string())),
        Err(e) => Err(KeyLoad::Unusable(format!("key loading task failed: {e}"))),
    }
}

/// Reads and decrypts a key file off the async runtime: the bcrypt KDF of an encrypted OpenSSH
/// key deliberately takes a noticeable fraction of a second.
async fn load_key(
    path: PathBuf,
    passphrase: Option<SecretString>,
) -> Result<russh::keys::PrivateKey, KeyLoad> {
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
            labels(&methods),
            ["agent", "key file", "password", "keyboard-interactive"]
        );
        assert!(matches!(
            methods.get(1),
            Some(AuthMethod::KeyFile(path)) if path == Path::new("/home/u/.ssh/id_ed25519")
        ));
    }

    fn labels(methods: &[AuthMethod]) -> Vec<&'static str> {
        methods.iter().map(AuthMethod::label).collect()
    }

    #[test]
    fn saved_credentials_go_first_in_their_kind() {
        let key = StoredKey {
            label: "Laptop".into(),
            fingerprint: "SHA256:x".into(),
            private_key: SecretString::from("-----BEGIN OPENSSH PRIVATE KEY-----"),
            passphrase: Some(SecretString::from("hunter2")),
        };
        let methods = AuthMethod::with_saved(
            Some(key),
            Some(SecretString::from("pw")),
            [PathBuf::from("/k")],
        );
        assert_eq!(
            labels(&methods),
            [
                "saved key",
                "agent",
                "key file",
                "saved password",
                "password",
                "keyboard-interactive"
            ]
        );
        let debug = format!("{methods:?}");
        assert!(
            !debug.contains("hunter2") && !debug.contains("OPENSSH"),
            "{debug}"
        );
        assert_eq!(
            labels(&AuthMethod::with_saved(None, None, [])),
            ["agent", "password", "keyboard-interactive"]
        );
    }

    #[tokio::test]
    async fn the_passphrase_cache_expires_and_forgets() {
        let cache = PassphraseCache::new(Duration::from_millis(50));
        cache.insert("SHA256:a", SecretString::from("one"));
        let copy = cache.clone();
        assert_eq!(copy.get("SHA256:a").unwrap().expose_secret(), "one");
        assert!(cache.get("SHA256:b").is_none());
        cache.forget("SHA256:a");
        assert!(cache.get("SHA256:a").is_none());

        cache.insert("SHA256:a", SecretString::from("two"));
        tokio::time::sleep(Duration::from_millis(80)).await;
        // The timer has already wiped it from the map, not just hidden it.
        assert!(cache.entries.lock().unwrap().is_empty());
        assert!(cache.get("SHA256:a").is_none());

        cache.insert("k", SecretString::from("three"));
        cache.clear();
        assert!(cache.get("k").is_none());

        let off = PassphraseCache::new(Duration::ZERO);
        off.insert("k", SecretString::from("x"));
        assert!(off.get("k").is_none());
        assert!(!format!("{cache:?}").contains("three"));
    }

    #[tokio::test]
    async fn key_loading_outcomes() {
        assert!(load_key(fixture("id_ed25519"), None).await.is_ok());
        assert!(load_key(fixture("id_rsa"), None).await.is_ok());
        assert!(matches!(
            load_key(fixture("does-not-exist"), None).await,
            Err(KeyLoad::Missing)
        ));
        let encrypted = fixture("id_ed25519_encrypted");
        assert!(matches!(
            load_key(encrypted.clone(), None).await,
            Err(KeyLoad::NeedsPassphrase)
        ));
        let wrong = Some(SecretString::from("wrong"));
        assert!(matches!(
            load_key(encrypted.clone(), wrong).await,
            Err(KeyLoad::NeedsPassphrase)
        ));
        let right = Some(SecretString::from("fixture-passphrase"));
        assert!(load_key(encrypted, right).await.is_ok());
        assert!(matches!(
            load_key(fixture("README.md"), None).await,
            Err(KeyLoad::Unusable(_))
        ));
    }
}
