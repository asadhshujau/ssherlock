//! Numbered SQL migrations, applied in order and recorded in `schema_migrations`.
//!
//! Each file in `migrations/` is `NNNN_description.sql` and is listed in [`MIGRATIONS`] (a test
//! checks the two agree). A shipped migration is never edited; changes go in a new one.

use rusqlite::Connection;
use ssherlock_core::Timestamp;

use crate::StoreError;

/// One schema change.
pub(crate) struct Migration {
    /// Its number, from 1 with no gaps.
    pub version: u32,
    /// The description part of its file name.
    pub name: &'static str,
    /// The SQL.
    pub sql: &'static str,
}

/// Every migration, in order.
pub(crate) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "hosts",
    sql: include_str!("../migrations/0001_hosts.sql"),
}];

/// The newest schema version this build knows.
fn latest() -> u32 {
    MIGRATIONS.last().map_or(0, |m| m.version)
}

/// Brings the schema up to date.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    migrate_to(conn, latest())
}

/// Applies every migration up to and including `target`, each in its own transaction.
pub(crate) fn migrate_to(conn: &mut Connection, target: u32) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY NOT NULL,
            name        TEXT NOT NULL,
            applied_at  TEXT NOT NULL
        ) STRICT;",
    )?;
    let current = current_version(conn)?;
    if current > latest() {
        return Err(StoreError::NewerSchema {
            found: current,
            known: latest(),
        });
    }
    for migration in MIGRATIONS
        .iter()
        .filter(|m| m.version > current && m.version <= target)
    {
        let failed = |source| StoreError::Migration {
            version: migration.version,
            name: migration.name,
            source,
        };
        let tx = conn.transaction().map_err(failed)?;
        tx.execute_batch(migration.sql).map_err(failed)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            (
                migration.version,
                migration.name,
                Timestamp::now().to_string(),
            ),
        )
        .map_err(failed)?;
        tx.commit().map_err(failed)?;
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "applied a database migration"
        );
    }
    Ok(())
}

/// The highest migration applied, 0 for a new database.
pub(crate) fn current_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn the_list_matches_the_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        let listed: Vec<String> = MIGRATIONS
            .iter()
            .map(|m| format!("{:04}_{}.sql", m.version, m.name))
            .collect();
        assert_eq!(files, listed);
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                migration.version as usize,
                index + 1,
                "versions must have no gaps"
            );
        }
    }

    #[test]
    fn applies_cleanly_from_empty_and_again_as_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), latest());
        assert_eq!(
            tables(&conn),
            ["groups", "host_tags", "hosts", "schema_migrations", "trash"]
        );
        migrate(&mut conn).unwrap();
        let applied: u32 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, latest());
        let problems: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(problems, Vec::<String>::new());
    }

    /// Every earlier version upgrades to the latest: the path an existing user's database takes.
    #[test]
    fn upgrades_from_every_earlier_version() {
        for from in 0..latest() {
            let mut conn = Connection::open_in_memory().unwrap();
            migrate_to(&mut conn, from).unwrap();
            assert_eq!(current_version(&conn).unwrap(), from);
            migrate(&mut conn).unwrap();
            assert_eq!(current_version(&conn).unwrap(), latest(), "from {from}");
        }
    }

    #[test]
    fn a_newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', '')",
            [latest() + 1],
        )
        .unwrap();
        match migrate(&mut conn) {
            Err(StoreError::NewerSchema { found, known }) => {
                assert_eq!((found, known), (latest() + 1, latest()));
            }
            other => panic!("expected NewerSchema, got {other:?}"),
        }
    }
}
