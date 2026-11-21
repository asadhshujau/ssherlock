//! The store's one error type.

use std::path::PathBuf;

use ssherlock_core::forwarding::ForwardRuleError;
use ssherlock_core::hosts::{GroupError, HostError};
use ssherlock_core::keychain::{IdentityError, KeyError};

use crate::KeyLocation;

/// Why a store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The database file couldn't be opened or created.
    #[error("could not open the database at {path}")]
    Open {
        /// The database file.
        path: PathBuf,
        /// What SQLite said.
        #[source]
        source: rusqlite::Error,
    },
    /// The directory for the database file couldn't be created.
    #[error("could not create the directory {path}")]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// A migration failed; the database is left at the version before it.
    #[error("could not update the database to version {version} ({name})")]
    Migration {
        /// The migration's number.
        version: u32,
        /// The migration's name.
        name: &'static str,
        /// What SQLite said.
        #[source]
        source: rusqlite::Error,
    },
    /// The database was last used by a newer SSHerlock that knows more migrations.
    #[error(
        "the database is at version {found} but this SSHerlock only knows up to {known}; \
         update SSHerlock to open it"
    )]
    NewerSchema {
        /// The database's version.
        found: u32,
        /// The newest version this build knows.
        known: u32,
    },
    /// A query failed.
    #[error("could not query the database")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored value couldn't be read back, e.g. an id that isn't a UUID.
    #[error("could not read {column} of {table} row {id}: {reason}")]
    Corrupt {
        /// The table.
        table: &'static str,
        /// The row's id (as stored).
        id: String,
        /// The column.
        column: &'static str,
        /// What was wrong with it.
        reason: String,
    },
    /// A value couldn't be turned into JSON for storing.
    #[error("could not encode {what} as JSON")]
    Encode {
        /// What was being encoded.
        what: &'static str,
        /// What serde said.
        #[source]
        source: serde_json::Error,
    },
    /// A host failed validation and wasn't saved.
    #[error("could not save the host")]
    InvalidHost(#[from] HostError),
    /// A group failed validation, or would end up inside itself, and wasn't saved.
    #[error("could not save the group")]
    InvalidGroup(#[from] GroupError),
    /// A key failed validation and wasn't saved.
    #[error("could not save the key")]
    InvalidKey(#[from] KeyError),
    /// An identity failed validation and wasn't saved.
    #[error("could not save the identity")]
    InvalidIdentity(#[from] IdentityError),
    /// A port-forwarding rule failed validation and wasn't saved.
    #[error("could not save the port forwarding rules")]
    InvalidForwardRule(#[from] ForwardRuleError),
    /// A port-forwarding rule was saved with a host it doesn't belong to (a bug in the caller).
    #[error("the port forwarding rule {rule} belongs to another host")]
    ForeignForwardRule {
        /// The rule's id.
        rule: String,
    },
    /// The trash entry to restore doesn't exist (it was already restored or emptied).
    #[error("the trash entry {0} no longer exists")]
    TrashEntryGone(String),
    /// Something to move to the trash doesn't exist.
    #[error("there is no {what} with id {id} to delete")]
    NotFound {
        /// What kind of record.
        what: &'static str,
        /// Its id.
        id: String,
    },
    /// The OS keychain refused or failed a request for the vault key.
    #[error("could not {action} the vault key in the OS keychain")]
    Keychain {
        /// What was being done: find, read or save.
        action: &'static str,
        /// What the keychain said.
        #[source]
        source: keyring::Error,
    },
    /// The vault key file couldn't be read or written.
    #[error("could not use the vault key file at {path}")]
    KeyFile {
        /// The key file.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// The place this database's vault key was kept no longer has it.
    #[error("the vault key is missing from {0}, so saved passwords and keys cannot be opened")]
    VaultKeyMissing(KeyLocation),
    /// The vault key found doesn't open this database's secrets.
    #[error("the vault key in {0} does not belong to this database")]
    VaultWrongKey(KeyLocation),
    /// The stored vault key isn't 256 bits long.
    #[error("the vault key has the wrong length")]
    VaultKeyInvalid,
    /// The OS couldn't supply random bytes for a key or nonce.
    #[error("could not get random bytes from the operating system")]
    Random(#[source] rand::rngs::SysError),
    /// Encrypting a secret failed.
    #[error("could not encrypt a secret")]
    Seal,
    /// A saved secret couldn't be decrypted: damaged, or sealed by another vault.
    #[error("could not decrypt a saved secret; it may be damaged")]
    Unseal,
    /// A thread panicked while holding the database lock; the connection may be mid-way through
    /// something, so it is no longer used.
    #[error("the database is unavailable after an earlier crash; restart SSHerlock")]
    Poisoned,
}
