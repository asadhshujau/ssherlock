//! The trash: its [`Repository`](crate::Repository) implementation, restoring, and emptying
//! (HOST-09).
//!
//! **Store** layer. Entries hold JSON copies of what was deleted; see
//! `ssherlock_core::trash`.

use rusqlite::{Connection, OptionalExtension, Row, params};
use ssherlock_core::Id;
use ssherlock_core::hosts::{Group, Host};
use ssherlock_core::trash::{TrashContents, TrashEntry};

use crate::rows::{RowReader, Rows, to_json};
use crate::{Store, StoreError};

fn read(row: &Row<'_>) -> rusqlite::Result<(String, String, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn into_entry(
    (id, deleted_at, contents): (String, String, String),
) -> Result<TrashEntry, StoreError> {
    let r = RowReader {
        table: "trash",
        id: &id,
    };
    Ok(TrashEntry {
        id: r.id()?,
        deleted_at: r.time("deleted_at", &deleted_at)?,
        contents: r.json::<TrashContents>("contents", &contents)?,
    })
}

impl Rows for TrashEntry {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        conn.prepare_cached("SELECT id, deleted_at, contents FROM trash WHERE id = ?1")?
            .query_row([id.to_string()], read)
            .optional()?
            .map(into_entry)
            .transpose()
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut stmt = conn.prepare_cached("SELECT id, deleted_at, contents FROM trash")?;
        let rows = stmt.query_map([], read)?;
        rows.map(|row| into_entry(row?)).collect()
    }

    fn upsert(conn: &Connection, entry: &Self) -> Result<(), StoreError> {
        conn.prepare_cached(
            "INSERT INTO trash (id, deleted_at, contents) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET
                deleted_at = excluded.deleted_at,
                contents = excluded.contents",
        )?
        .execute(params![
            entry.id.to_string(),
            entry.deleted_at.to_string(),
            to_json("trash contents", &entry.contents)?,
        ])?;
        Ok(())
    }

    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        conn.prepare_cached("DELETE FROM trash WHERE id = ?1")?
            .execute([id.to_string()])?;
        Ok(())
    }
}

impl Store {
    /// Puts everything in the entry back and removes it from the trash. A group or host whose
    /// parent group is gone by now is restored at the top level.
    ///
    /// # Errors
    ///
    /// [`StoreError::TrashEntryGone`] if the entry doesn't exist, or a database error.
    pub fn restore(&self, entry: Id<TrashEntry>) -> Result<(), StoreError> {
        self.write(|tx| {
            let found = TrashEntry::get(tx, entry)?
                .ok_or_else(|| StoreError::TrashEntryGone(entry.to_string()))?;
            let exists =
                |id: Id<Group>| -> Result<bool, StoreError> { Ok(Group::get(tx, id)?.is_some()) };
            // Groups are stored parents first, so each parent is back before its children.
            for group in found.contents.groups {
                let parent = match group.parent {
                    Some(parent) if exists(parent)? => Some(parent),
                    _ => None,
                };
                Group::upsert(tx, &Group { parent, ..group })?;
            }
            for host in found.contents.hosts {
                let group = match host.group {
                    Some(group) if exists(group)? => Some(group),
                    _ => None,
                };
                Host::upsert(tx, &Host { group, ..host })?;
            }
            TrashEntry::delete(tx, entry)
        })
    }

    /// Deletes every trash entry for good.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be written.
    pub fn empty_trash(&self) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.execute("DELETE FROM trash", [])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::Timestamp;
    use ssherlock_core::hosts::Tag;

    use super::*;
    use crate::Repository;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    #[test]
    fn a_trashed_host_comes_back_exactly() {
        let store = Store::open_in_memory().unwrap();
        let mut h = Host::new("example.com".parse().unwrap(), at(1));
        h.tags = Tag::parse_list("prod").unwrap();
        store.upsert(&h).unwrap();
        store.record_connection(h.id, at(7)).unwrap();
        let h: Host = store.get(h.id).unwrap().unwrap();

        let [entry] = store.trash_hosts(&[h.id], at(9)).unwrap()[..] else {
            unreachable!("one host makes one entry")
        };
        store.restore(entry).unwrap();

        assert_eq!(store.get(h.id).unwrap(), Some(h));
        assert_eq!(Repository::<TrashEntry>::list(&store).unwrap(), Vec::new());
        assert!(matches!(
            store.restore(entry),
            Err(StoreError::TrashEntryGone(_))
        ));
    }

    #[test]
    fn a_trashed_group_comes_back_with_everything_inside() {
        let store = Store::open_in_memory().unwrap();
        let prod = Group::new("prod", None, at(1));
        let db = Group::new("db", Some(prod.id), at(1));
        store.upsert_all(&[prod.clone(), db.clone()]).unwrap();
        let mut h = Host::new("db1".parse().unwrap(), at(1));
        h.group = Some(db.id);
        store.upsert(&h).unwrap();

        let entry = store.trash_group(prod.id, at(9)).unwrap();
        store.restore(entry).unwrap();

        assert_eq!(store.get(prod.id).unwrap(), Some(prod));
        assert_eq!(store.get(db.id).unwrap(), Some(db));
        assert_eq!(store.get(h.id).unwrap(), Some(h));
    }

    #[test]
    fn restoring_into_a_deleted_group_lands_at_the_top() {
        let store = Store::open_in_memory().unwrap();
        let outer = Group::new("outer", None, at(1));
        let inner = Group::new("inner", Some(outer.id), at(1));
        store.upsert_all(&[outer.clone(), inner.clone()]).unwrap();
        let mut h = Host::new("h".parse().unwrap(), at(1));
        h.group = Some(outer.id);
        store.upsert(&h).unwrap();

        let host_entry = store.trash_hosts(&[h.id], at(2)).unwrap();
        let inner_entry = store.trash_group(inner.id, at(3)).unwrap();
        Repository::<Group>::delete(&store, outer.id).unwrap();

        store.restore(inner_entry).unwrap();
        for entry in host_entry {
            store.restore(entry).unwrap();
        }
        assert_eq!(
            store.get(inner.id).unwrap().map(|g: Group| g.parent),
            Some(None)
        );
        assert_eq!(store.get(h.id).unwrap().map(|h: Host| h.group), Some(None));
    }

    #[test]
    fn emptying_and_permanent_delete() {
        let store = Store::open_in_memory().unwrap();
        let a = Host::new("a".parse().unwrap(), at(1));
        let b = Host::new("b".parse().unwrap(), at(1));
        store.upsert_all(&[a.clone(), b.clone()]).unwrap();
        let entries = store.trash_hosts(&[a.id, b.id], at(2)).unwrap();

        Repository::<TrashEntry>::delete(&store, *entries.first().unwrap()).unwrap();
        assert_eq!(Repository::<TrashEntry>::list(&store).unwrap().len(), 1);
        store.empty_trash().unwrap();
        assert_eq!(Repository::<TrashEntry>::list(&store).unwrap(), Vec::new());
        assert_eq!(Repository::<Host>::list(&store).unwrap(), Vec::new());
    }
}
