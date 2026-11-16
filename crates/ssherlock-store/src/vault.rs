//! The vault: seals secrets before they reach the database and opens them again (SEC-01).
//!
//! **Store** layer. Every saved password, passphrase and private key is sealed with
//! XChaCha20-Poly1305 under one random 256-bit master key, with a fresh random nonce each time.
//! The master key lives in the OS keychain ([`OsKeychain`]), or in a key file readable only by
//! the user ([`KeyFile`]) where there is no keychain. The database records which, so a later
//! start finds the same key or says why it can't (ADR 0008).

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::Aead as _;
use chacha20poly1305::{KeyInit as _, XChaCha20Poly1305, XNonce};
use rand::TryRng as _;
use rusqlite::OptionalExtension as _;
use ssherlock_core::{ExposeSecret, Id, Sealed, SecretBox, SecretString, Timestamp, app};
use zeroize::Zeroizing;

use crate::{Store, StoreError};

/// The first byte of every sealed value: the format, so it can change later.
const FORMAT_V1: u8 = 1;
/// `XChaCha20`'s nonce size. 192 bits is enough that random nonces never repeat in practice.
const NONCE_LEN: usize = 24;
/// The master key's size: 256 bits.
const KEY_LEN: usize = 32;
/// Authenticated with every sealed value so ciphertext from anything else can't pass as ours.
const AAD: &[u8] = b"ssherlock-vault-v1";
/// Sealed into `vault.check_value`; opening it proves the key is the right one.
const CHECK_PLAINTEXT: &[u8] = b"ssherlock vault check v1";

/// Opens and seals secrets with the master key. Made by [`Store::unlock_vault`].
///
/// Cheap to share behind an `Arc`. The key is wiped from memory when the vault is dropped.
pub struct Vault {
    cipher: XChaCha20Poly1305,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault")
    }
}

impl Vault {
    fn new(key: &[u8]) -> Result<Self, StoreError> {
        XChaCha20Poly1305::new_from_slice(key)
            .map(|cipher| Self { cipher })
            .map_err(|_| StoreError::VaultKeyInvalid)
    }

    /// Encrypts `secret`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Random`] if the OS can't supply a nonce, [`StoreError::Seal`] if
    /// encryption fails.
    pub fn seal(&self, secret: &[u8]) -> Result<Sealed, StoreError> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::SysRng
            .try_fill_bytes(&mut nonce)
            .map_err(StoreError::Random)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                chacha20poly1305::aead::Payload {
                    msg: secret,
                    aad: AAD,
                },
            )
            .map_err(|_| StoreError::Seal)?;
        let mut bytes = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        bytes.push(FORMAT_V1);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ciphertext);
        Ok(Sealed::from_bytes(bytes))
    }

    /// Encrypts a text secret such as a password.
    ///
    /// # Errors
    ///
    /// As for [`seal`](Self::seal).
    pub fn seal_str(&self, secret: &SecretString) -> Result<Sealed, StoreError> {
        self.seal(secret.expose_secret().as_bytes())
    }

    /// Decrypts `sealed`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Unseal`] if it wasn't sealed by this vault or has been changed.
    pub fn open(&self, sealed: &Sealed) -> Result<SecretBox<[u8]>, StoreError> {
        let (format, rest) = sealed.as_bytes().split_first().ok_or(StoreError::Unseal)?;
        if *format != FORMAT_V1 {
            return Err(StoreError::Unseal);
        }
        let (nonce, ciphertext) = rest.split_at_checked(NONCE_LEN).ok_or(StoreError::Unseal)?;
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| StoreError::Unseal)?;
        let plaintext = self
            .cipher
            .decrypt(
                &XNonce::from(nonce),
                chacha20poly1305::aead::Payload {
                    msg: ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| StoreError::Unseal)?;
        Ok(SecretBox::new(plaintext.into_boxed_slice()))
    }

    /// Decrypts a text secret sealed with [`seal_str`](Self::seal_str).
    ///
    /// # Errors
    ///
    /// [`StoreError::Unseal`] as for [`open`](Self::open), or if the plaintext isn't UTF-8.
    pub fn open_string(&self, sealed: &Sealed) -> Result<SecretString, StoreError> {
        let bytes = Zeroizing::new(self.open(sealed)?.expose_secret().to_vec());
        let text = std::str::from_utf8(&bytes).map_err(|_| StoreError::Unseal)?;
        Ok(SecretString::from(text))
    }
}

/// Where the master key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyLocation {
    /// The OS keychain: Keychain on macOS, the Secret Service (GNOME Keyring, `KWallet`) on Linux.
    Keychain,
    /// A file next to the database that only the user can read. Used where there is no
    /// keychain; weaker, because anyone who can read the user's files can read the key.
    File,
}

impl KeyLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::File => "file",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "keychain" => Some(Self::Keychain),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

impl fmt::Display for KeyLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Keychain => "the OS keychain",
            Self::File => "the vault key file",
        })
    }
}

/// Somewhere a master key can be kept. Implemented by [`OsKeychain`], [`KeyFile`] and, for
/// tests, [`MemoryKeyHolder`].
///
/// Every call may block (the keychain can show an unlock dialog), so call these from a
/// background thread.
pub trait KeyHolder {
    /// Which kind of place this is.
    fn location(&self) -> KeyLocation;

    /// The key saved for `vault`, or `None` if there isn't one.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the place can't be read.
    fn load(&self, vault: &str) -> Result<Option<SecretBox<[u8]>>, StoreError>;

    /// Saves `key` for `vault`.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the place can't be written.
    fn save(&self, vault: &str, key: &[u8]) -> Result<(), StoreError>;
}

/// The OS keychain. Each database's key is a separate entry named after its vault id, so test
/// databases and real ones never share a key.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsKeychain;

impl OsKeychain {
    fn entry(vault: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(app::ID, &format!("vault-{vault}")).map_err(|source| {
            StoreError::Keychain {
                action: "find",
                source,
            }
        })
    }
}

impl KeyHolder for OsKeychain {
    fn location(&self) -> KeyLocation {
        KeyLocation::Keychain
    }

    fn load(&self, vault: &str) -> Result<Option<SecretBox<[u8]>>, StoreError> {
        match Self::entry(vault)?.get_secret() {
            Ok(key) => Ok(Some(SecretBox::new(key.into_boxed_slice()))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(source) => Err(StoreError::Keychain {
                action: "read",
                source,
            }),
        }
    }

    fn save(&self, vault: &str, key: &[u8]) -> Result<(), StoreError> {
        Self::entry(vault)?
            .set_secret(key)
            .map_err(|source| StoreError::Keychain {
                action: "save",
                source,
            })
    }
}

/// A key file only the user can read, for platforms or sessions without a keychain.
#[derive(Clone, Debug)]
pub struct KeyFile {
    path: PathBuf,
}

impl KeyFile {
    /// The key file at `path`, usually `vault.key` next to the database.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where the file is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn error(&self, source: std::io::Error) -> StoreError {
        StoreError::KeyFile {
            path: self.path.clone(),
            source,
        }
    }
}

impl KeyHolder for KeyFile {
    fn location(&self) -> KeyLocation {
        KeyLocation::File
    }

    /// The file holds one key; `vault` is ignored because the file sits beside its database.
    fn load(&self, _vault: &str) -> Result<Option<SecretBox<[u8]>>, StoreError> {
        match std::fs::read(&self.path) {
            Ok(key) => Ok(Some(SecretBox::new(key.into_boxed_slice()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(self.error(e)),
        }
    }

    fn save(&self, _vault: &str, key: &[u8]) -> Result<(), StoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.error(e))?;
        }
        let mut options = std::fs::OpenOptions::new();
        // `create_new`: never overwrite a key, which would lose every secret sealed with it.
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&self.path).map_err(|e| self.error(e))?;
        file.write_all(key)
            .and_then(|()| file.sync_all())
            .map_err(|e| self.error(e))
    }
}

/// Keeps keys in memory only, so secrets sealed under them are lost when it is dropped. For
/// tests, and for a keychain that should behave as missing.
#[derive(Debug, Default)]
pub struct MemoryKeyHolder {
    keys: std::sync::Mutex<std::collections::HashMap<String, Zeroizing<Vec<u8>>>>,
    is_unavailable: bool,
}

impl MemoryKeyHolder {
    /// A holder that refuses every call, like a session without a keychain.
    pub fn unavailable() -> Self {
        Self {
            is_unavailable: true,
            ..Self::default()
        }
    }

    fn check(&self) -> Result<(), StoreError> {
        if self.is_unavailable {
            Err(StoreError::Keychain {
                action: "find",
                source: keyring::Error::NoStorageAccess(
                    std::io::Error::other("no keychain in this test").into(),
                ),
            })
        } else {
            Ok(())
        }
    }
}

impl KeyHolder for MemoryKeyHolder {
    fn location(&self) -> KeyLocation {
        KeyLocation::Keychain
    }

    fn load(&self, vault: &str) -> Result<Option<SecretBox<[u8]>>, StoreError> {
        self.check()?;
        let keys = self.keys.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(keys
            .get(vault)
            .map(|key| SecretBox::new(key.to_vec().into_boxed_slice())))
    }

    fn save(&self, vault: &str, key: &[u8]) -> Result<(), StoreError> {
        self.check()?;
        let mut keys = self.keys.lock().map_err(|_| StoreError::Poisoned)?;
        keys.insert(vault.to_owned(), Zeroizing::new(key.to_vec()));
        Ok(())
    }
}

/// The vault and where its key was found, from [`Store::unlock_vault`].
#[derive(Debug)]
pub struct Unlocked {
    /// The vault.
    pub vault: Vault,
    /// Where its key is kept, so the UI can warn when it's only a file.
    pub location: KeyLocation,
    /// Whether the vault was created just now, rather than found from an earlier start.
    pub is_new: bool,
}

/// The kind of thing an [`Id`] in the `vault` table names.
struct VaultTag;

impl Store {
    /// Opens the vault: loads the master key from wherever this database recorded it, or, the
    /// first time, makes one and keeps it in `keychain`, else in `fallback`.
    ///
    /// Blocks while the keychain answers, which can include an unlock dialog. Call it from a
    /// background thread.
    ///
    /// # Errors
    ///
    /// [`StoreError::VaultKeyMissing`] if the recorded place has no key,
    /// [`StoreError::VaultWrongKey`] if its key doesn't match, a keychain or key file error if
    /// neither place can be used, or a database error.
    pub fn unlock_vault(
        &self,
        keychain: &dyn KeyHolder,
        fallback: &dyn KeyHolder,
    ) -> Result<Unlocked, StoreError> {
        let existing = self.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT vault_id, key_location, check_value FROM vault WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    },
                )
                .optional()?)
        })?;
        match existing {
            Some((vault_id, location, check)) => {
                let location =
                    KeyLocation::parse(&location).ok_or_else(|| StoreError::Corrupt {
                        table: "vault",
                        id: vault_id.clone(),
                        column: "key_location",
                        reason: format!("{location:?} is not a key location"),
                    })?;
                let holder = if location == keychain.location() {
                    keychain
                } else {
                    fallback
                };
                let key = holder
                    .load(&vault_id)?
                    .ok_or(StoreError::VaultKeyMissing(location))?;
                let vault = Vault::new(key.expose_secret())?;
                let opened = vault
                    .open(&Sealed::from_bytes(check))
                    .map_err(|_| StoreError::VaultWrongKey(location))?;
                if opened.expose_secret() != CHECK_PLAINTEXT {
                    return Err(StoreError::VaultWrongKey(location));
                }
                tracing::info!(%location, "unlocked the vault");
                Ok(Unlocked {
                    vault,
                    location,
                    is_new: false,
                })
            }
            None => self.create_vault(keychain, fallback),
        }
    }

    fn create_vault(
        &self,
        keychain: &dyn KeyHolder,
        fallback: &dyn KeyHolder,
    ) -> Result<Unlocked, StoreError> {
        let vault_id = Id::<VaultTag>::generate().to_string();
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        rand::rngs::SysRng
            .try_fill_bytes(key.as_mut())
            .map_err(StoreError::Random)?;
        let location = match save_and_verify(keychain, &vault_id, key.as_ref()) {
            Ok(()) => keychain.location(),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "the OS keychain is unavailable, so the vault key goes in a file instead"
                );
                save_and_verify(fallback, &vault_id, key.as_ref())?;
                fallback.location()
            }
        };
        let vault = Vault::new(key.as_ref())?;
        let check = vault.seal(CHECK_PLAINTEXT)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO vault (id, vault_id, key_location, check_value, created_at)
                 VALUES (1, ?1, ?2, ?3, ?4)",
                (
                    &vault_id,
                    location.as_str(),
                    check.as_bytes(),
                    Timestamp::now().to_string(),
                ),
            )?;
            Ok(())
        })?;
        tracing::info!(%location, "created the vault");
        Ok(Unlocked {
            vault,
            location,
            is_new: true,
        })
    }
}

/// Saves `key` and reads it back, so a keychain that accepts a write but keeps nothing (or
/// keeps it only for this login) isn't trusted with the only copy.
fn save_and_verify(holder: &dyn KeyHolder, vault: &str, key: &[u8]) -> Result<(), StoreError> {
    holder.save(vault, key)?;
    match holder.load(vault)? {
        Some(read) if read.expose_secret() == key => Ok(()),
        _ => Err(StoreError::VaultKeyMissing(holder.location())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use pretty_assertions::assert_eq;

    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("ssherlock-vault-{}", Id::<()>::generate()))
            .join(name)
    }

    fn unlocked() -> (Store, MemoryKeyHolder, Unlocked) {
        let store = Store::open_in_memory().unwrap();
        let keychain = MemoryKeyHolder::default();
        let fallback = MemoryKeyHolder::unavailable();
        let unlocked = store.unlock_vault(&keychain, &fallback).unwrap();
        (store, keychain, unlocked)
    }

    #[test]
    fn seal_and_open_round_trip_without_the_plaintext_in_the_ciphertext() {
        let (
            _,
            _,
            Unlocked {
                vault, location, ..
            },
        ) = unlocked();
        assert_eq!(location, KeyLocation::Keychain);
        let secret = b"-----BEGIN OPENSSH PRIVATE KEY----- hunter2";
        let sealed = vault.seal(secret).unwrap();
        assert!(
            !sealed
                .as_bytes()
                .windows(7)
                .any(|w| w == b"hunter2" || w == b"OPENSSH")
        );
        assert_eq!(vault.open(&sealed).unwrap().expose_secret(), secret);
        // A fresh nonce each time: the same secret never seals to the same bytes.
        assert_ne!(vault.seal(secret).unwrap(), sealed);

        let password = SecretString::from("correct horse");
        let sealed = vault.seal_str(&password).unwrap();
        assert_eq!(
            vault.open_string(&sealed).unwrap().expose_secret(),
            "correct horse"
        );
    }

    #[test]
    fn tampered_truncated_or_foreign_ciphertext_does_not_open() {
        let (_, _, Unlocked { vault, .. }) = unlocked();
        let sealed = vault.seal(b"secret").unwrap();
        let mut flipped = sealed.as_bytes().to_vec();
        if let Some(last) = flipped.last_mut() {
            *last ^= 1;
        }
        let mut wrong_format = sealed.as_bytes().to_vec();
        wrong_format[0] = 2;
        let (_, _, Unlocked { vault: other, .. }) = unlocked();
        let cases = [
            ("flipped bit", Sealed::from_bytes(flipped)),
            ("unknown format", Sealed::from_bytes(wrong_format)),
            ("empty", Sealed::from_bytes(Vec::new())),
            (
                "nonce only",
                Sealed::from_bytes(sealed.as_bytes()[..20].to_vec()),
            ),
        ];
        for (name, bad) in cases {
            assert!(
                matches!(vault.open(&bad), Err(StoreError::Unseal)),
                "{name}"
            );
        }
        assert!(matches!(other.open(&sealed), Err(StoreError::Unseal)));
    }

    #[test]
    fn the_same_key_is_found_again_on_the_next_start() {
        let (store, keychain, Unlocked { vault, .. }) = unlocked();
        let sealed = vault.seal(b"kept").unwrap();
        let fallback = MemoryKeyHolder::unavailable();
        let again = store.unlock_vault(&keychain, &fallback).unwrap();
        assert_eq!(again.vault.open(&sealed).unwrap().expose_secret(), b"kept");
    }

    #[test]
    fn without_a_keychain_the_key_goes_in_a_private_file() {
        let path = temp_path("vault.key");
        let store = Store::open_in_memory().unwrap();
        let no_keychain = MemoryKeyHolder::unavailable();
        let file = KeyFile::new(&path);
        let unlocked = store.unlock_vault(&no_keychain, &file).unwrap();
        assert_eq!(unlocked.location, KeyLocation::File);
        assert!(unlocked.is_new);
        assert_eq!(std::fs::read(&path).unwrap().len(), KEY_LEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let sealed = unlocked.vault.seal(b"x").unwrap();

        // Next start: the keychain works now, but the database says the key is in the file.
        let keychain = MemoryKeyHolder::default();
        let again = store.unlock_vault(&keychain, &file).unwrap();
        assert_eq!(again.location, KeyLocation::File);
        assert!(!again.is_new);
        assert_eq!(again.vault.open(&sealed).unwrap().expose_secret(), b"x");
        assert!(keychain.load("anything").unwrap().is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_or_replaced_key_is_an_error_not_a_new_key() {
        let (store, keychain, _) = unlocked();
        let fallback = MemoryKeyHolder::default();

        // The keychain is unavailable on a later start: fail rather than make a second key.
        let gone = MemoryKeyHolder::unavailable();
        assert!(matches!(
            store.unlock_vault(&gone, &fallback),
            Err(StoreError::Keychain { .. })
        ));
        // The keychain lost the entry.
        let empty = MemoryKeyHolder::default();
        assert!(matches!(
            store.unlock_vault(&empty, &fallback),
            Err(StoreError::VaultKeyMissing(KeyLocation::Keychain))
        ));
        // The entry holds a different key.
        let vault_id: String = store
            .read(|c| Ok(c.query_row("SELECT vault_id FROM vault", [], |r| r.get(0))?))
            .unwrap();
        let replaced = MemoryKeyHolder::default();
        replaced.save(&vault_id, &[7u8; KEY_LEN]).unwrap();
        assert!(matches!(
            store.unlock_vault(&replaced, &fallback),
            Err(StoreError::VaultWrongKey(KeyLocation::Keychain))
        ));
        // Nothing above disturbed the real key.
        assert!(store.unlock_vault(&keychain, &fallback).is_ok());
    }

    #[test]
    fn a_key_file_is_never_overwritten() {
        let path = temp_path("vault.key");
        let file = KeyFile::new(&path);
        file.save("v", &[1u8; KEY_LEN]).unwrap();
        assert!(matches!(
            file.save("v", &[2u8; KEY_LEN]),
            Err(StoreError::KeyFile { .. })
        ));
        assert_eq!(
            file.load("v").unwrap().unwrap().expose_secret(),
            &[1u8; KEY_LEN]
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn debug_output_of_the_vault_shows_nothing() {
        let (_, _, unlocked) = unlocked();
        assert_eq!(
            format!("{unlocked:?}"),
            "Unlocked { vault: Vault, location: Keychain, is_new: true }"
        );
    }
}
