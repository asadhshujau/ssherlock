//! The database connection and how operations get at it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, Transaction};

use crate::{StoreError, migrations};

/// SSHerlock's database. Cheap to share behind an `Arc`; calls from several threads take turns
/// on the one connection.
///
/// Every method blocks while SQLite works, typically well under a millisecond for the data a
/// desktop app holds. Call them from a background thread, never while rendering.
#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (creating if needed) the database at `path`, and its directory, and brings its
    /// schema up to date.
    ///
    /// # Errors
    ///
    /// [`StoreError::Open`] if the file can't be opened, [`StoreError::Migration`] or
    /// [`StoreError::NewerSchema`] if the schema can't be brought up to date.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| StoreError::CreateDir {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let open_error = |source| StoreError::Open {
            path: path.to_path_buf(),
            source,
        };
        let conn = Connection::open(path).map_err(open_error)?;
        // WAL lets readers carry on while a write commits; NORMAL sync is safe with WAL and
        // only risks the last transaction on power loss, never corruption.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(open_error)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(open_error)?;
        Self::from_connection(conn).inspect(|_| {
            tracing::info!(path = %path.display(), "opened the database");
        })
    }

    /// A fresh database in memory, for tests.
    ///
    /// # Errors
    ///
    /// As for [`Store::open`].
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(|source| StoreError::Open {
            path: ":memory:".into(),
            source,
        })?;
        Self::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Another SSHerlock window or process holding the lock gets a few seconds to finish
        // rather than failing straight away.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Runs `read` on the connection.
    pub(crate) fn read<R>(
        &self,
        read: impl FnOnce(&Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let conn = self.lock()?;
        read(&conn)
    }

    /// Runs `write` in a transaction, committed only if it returns `Ok`.
    pub(crate) fn write<R>(
        &self,
        write: impl FnOnce(&Transaction<'_>) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let result = write(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn.lock().map_err(|_| StoreError::Poisoned)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use ssherlock_core::hosts::Host;
    use ssherlock_core::{Id, Timestamp};

    use super::*;
    use crate::Repository;

    /// A directory under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("ssherlock-store-{}", Id::<()>::generate()));
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_database_is_created_with_its_directory_and_keeps_data() {
        let dir = TempDir::new();
        let path = dir.0.join("nested").join("ssherlock.db");
        let host = Host::new("example.com".parse().unwrap(), Timestamp::UNIX_EPOCH);
        {
            let store = Store::open(&path).unwrap();
            store.upsert(&host).unwrap();
            let mode: String = store
                .read(|c| Ok(c.pragma_query_value(None, "journal_mode", |r| r.get(0))?))
                .unwrap();
            assert_eq!(mode, "wal");
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.get(host.id).unwrap(), Some(host));
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().unwrap();
        let on: i64 = store
            .read(|c| Ok(c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?))
            .unwrap();
        assert_eq!(on, 1);
    }

    #[test]
    fn a_failed_write_commits_nothing() {
        let store = Store::open_in_memory().unwrap();
        let host = Host::new("example.com".parse().unwrap(), Timestamp::UNIX_EPOCH);
        let result: Result<(), StoreError> = store.write(|tx| {
            <Host as crate::rows::Rows>::upsert(tx, &host)?;
            Err(StoreError::Poisoned)
        });
        assert!(result.is_err());
        assert_eq!(Repository::<Host>::list(&store).unwrap(), Vec::new());
    }
}
