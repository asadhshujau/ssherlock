//! Hosts and groups: their [`Repository`](crate::Repository) implementations, recording
//! connections, and moving them to the trash.
//!
//! **Store** layer of the hosts feature. Tags live in `host_tags`; settings are JSON.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, params};
use ssherlock_core::hosts::{Group, GroupTree, Host, Tag};
use ssherlock_core::trash::{TrashContents, TrashEntry};
use ssherlock_core::{Id, Timestamp};

use crate::rows::{RowReader, Rows, to_json};
use crate::{Store, StoreError};

const HOST_COLUMNS: &str = "id, label, address, group_id, notes, is_favourite, settings, \
                            created_at, updated_at, last_connected_at";

/// A `hosts` row as stored, before parsing.
struct HostRow {
    id: String,
    label: String,
    address: String,
    group_id: Option<String>,
    notes: String,
    is_favourite: bool,
    settings: String,
    created_at: String,
    updated_at: String,
    last_connected_at: Option<String>,
}

impl HostRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            label: row.get(1)?,
            address: row.get(2)?,
            group_id: row.get(3)?,
            notes: row.get(4)?,
            is_favourite: row.get(5)?,
            settings: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            last_connected_at: row.get(9)?,
        })
    }

    fn into_host(self, tags: BTreeSet<Tag>) -> Result<Host, StoreError> {
        let r = RowReader {
            table: "hosts",
            id: &self.id,
        };
        Ok(Host {
            id: r.id()?,
            label: self.label,
            address: r.parse("address", &self.address)?,
            group: r.parse_opt("group_id", self.group_id.as_deref())?,
            tags,
            notes: self.notes,
            is_favourite: self.is_favourite,
            settings: r.json("settings", &self.settings)?,
            created_at: r.time("created_at", &self.created_at)?,
            updated_at: r.time("updated_at", &self.updated_at)?,
            last_connected_at: r
                .parse_opt("last_connected_at", self.last_connected_at.as_deref())?,
        })
    }
}

/// Tags for one host, or for all of them with `host = None`, by host id text.
fn load_tags(
    conn: &Connection,
    host: Option<&str>,
) -> Result<HashMap<String, BTreeSet<Tag>>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT host_id, tag FROM host_tags WHERE ?1 IS NULL OR host_id = ?1")?;
    let mut tags: HashMap<String, BTreeSet<Tag>> = HashMap::new();
    for row in stmt.query_map([host], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (host_id, tag) = row?;
        let tag = RowReader {
            table: "host_tags",
            id: &host_id,
        }
        .parse("tag", &tag)?;
        tags.entry(host_id).or_default().insert(tag);
    }
    Ok(tags)
}

impl Rows for Host {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        let id = id.to_string();
        let row = conn
            .prepare_cached(&format!("SELECT {HOST_COLUMNS} FROM hosts WHERE id = ?1"))?
            .query_row([&id], HostRow::read)
            .optional()?;
        let Some(row) = row else { return Ok(None) };
        let tags = load_tags(conn, Some(&id))?.remove(&id).unwrap_or_default();
        row.into_host(tags).map(Some)
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut tags = load_tags(conn, None)?;
        let mut stmt = conn.prepare_cached(&format!("SELECT {HOST_COLUMNS} FROM hosts"))?;
        let rows = stmt.query_map([], HostRow::read)?;
        rows.map(|row| {
            let row = row?;
            let host_tags = tags.remove(&row.id).unwrap_or_default();
            row.into_host(host_tags)
        })
        .collect()
    }

    /// Saves everything except `last_connected_at` of an existing host, which only
    /// [`Store::record_connection`] changes. A new host (e.g. restored from the trash) keeps
    /// the one it has.
    fn upsert(conn: &Connection, host: &Self) -> Result<(), StoreError> {
        host.validate()?;
        let id = host.id.to_string();
        conn.prepare_cached(&format!(
            "INSERT INTO hosts ({HOST_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (id) DO UPDATE SET
                label = excluded.label,
                address = excluded.address,
                group_id = excluded.group_id,
                notes = excluded.notes,
                is_favourite = excluded.is_favourite,
                settings = excluded.settings,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at"
        ))?
        .execute(params![
            id,
            host.label,
            host.address.to_string(),
            host.group.map(|g| g.to_string()),
            host.notes,
            host.is_favourite,
            to_json("host settings", &host.settings)?,
            host.created_at.to_string(),
            host.updated_at.to_string(),
            host.last_connected_at.map(|t| t.to_string()),
        ])?;
        conn.prepare_cached("DELETE FROM host_tags WHERE host_id = ?1")?
            .execute([&id])?;
        let mut insert =
            conn.prepare_cached("INSERT INTO host_tags (host_id, tag) VALUES (?1, ?2)")?;
        for tag in &host.tags {
            insert.execute([&id, tag.as_str()])?;
        }
        Ok(())
    }

    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        conn.prepare_cached("DELETE FROM hosts WHERE id = ?1")?
            .execute([id.to_string()])?;
        Ok(())
    }
}

const GROUP_COLUMNS: &str = "id, name, parent_id, settings, created_at, updated_at";

/// A `groups` row as stored, before parsing.
struct GroupRow {
    id: String,
    name: String,
    parent_id: Option<String>,
    settings: String,
    created_at: String,
    updated_at: String,
}

impl GroupRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            parent_id: row.get(2)?,
            settings: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }

    fn into_group(self) -> Result<Group, StoreError> {
        let r = RowReader {
            table: "groups",
            id: &self.id,
        };
        Ok(Group {
            id: r.id()?,
            name: self.name,
            parent: r.parse_opt("parent_id", self.parent_id.as_deref())?,
            settings: r.json("settings", &self.settings)?,
            created_at: r.time("created_at", &self.created_at)?,
            updated_at: r.time("updated_at", &self.updated_at)?,
        })
    }
}

impl Rows for Group {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        conn.prepare_cached(&format!("SELECT {GROUP_COLUMNS} FROM groups WHERE id = ?1"))?
            .query_row([id.to_string()], GroupRow::read)
            .optional()?
            .map(GroupRow::into_group)
            .transpose()
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut stmt = conn.prepare_cached(&format!("SELECT {GROUP_COLUMNS} FROM groups"))?;
        let rows = stmt.query_map([], GroupRow::read)?;
        rows.map(|row| row?.into_group()).collect()
    }

    /// Refuses a group that is invalid or would end up inside itself.
    fn upsert(conn: &Connection, group: &Self) -> Result<(), StoreError> {
        group.validate()?;
        let groups = Self::list(conn)?;
        GroupTree::new(&groups).check_parent(group.id, group.parent)?;
        conn.prepare_cached(&format!(
            "INSERT INTO groups ({GROUP_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (id) DO UPDATE SET
                name = excluded.name,
                parent_id = excluded.parent_id,
                settings = excluded.settings,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at"
        ))?
        .execute(params![
            group.id.to_string(),
            group.name,
            group.parent.map(|p| p.to_string()),
            to_json("group settings", &group.settings)?,
            group.created_at.to_string(),
            group.updated_at.to_string(),
        ])?;
        Ok(())
    }

    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        conn.prepare_cached("DELETE FROM groups WHERE id = ?1")?
            .execute([id.to_string()])?;
        Ok(())
    }
}

impl Store {
    /// Notes that `host` was connected to at `at`, for Recents (HOST-07). A host that no longer
    /// exists is ignored: the connection may outlive its deletion.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be written.
    pub fn record_connection(&self, host: Id<Host>, at: Timestamp) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.prepare_cached("UPDATE hosts SET last_connected_at = ?2 WHERE id = ?1")?
                .execute([host.to_string(), at.to_string()])?;
            Ok(())
        })
    }

    /// Moves each host to the trash as its own entry, so each can be restored on its own.
    /// All or nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if a host doesn't exist, or a database error.
    pub fn trash_hosts(
        &self,
        hosts: &[Id<Host>],
        now: Timestamp,
    ) -> Result<Vec<Id<TrashEntry>>, StoreError> {
        self.write(|tx| {
            hosts
                .iter()
                .map(|&id| {
                    let host = Host::get(tx, id)?.ok_or_else(|| StoreError::NotFound {
                        what: "host",
                        id: id.to_string(),
                    })?;
                    let entry = TrashEntry::new(
                        TrashContents {
                            hosts: vec![host],
                            ..TrashContents::default()
                        },
                        now,
                    );
                    TrashEntry::upsert(tx, &entry)?;
                    Host::delete(tx, id)?;
                    Ok(entry.id)
                })
                .collect()
        })
    }

    /// Moves `group`, every group inside it and all their hosts to the trash as one entry.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] if the group doesn't exist, or a database error.
    pub fn trash_group(
        &self,
        group: Id<Group>,
        now: Timestamp,
    ) -> Result<Id<TrashEntry>, StoreError> {
        self.write(|tx| {
            let all_groups = Group::list(tx)?;
            let tree = GroupTree::new(&all_groups);
            if tree.get(group).is_none() {
                return Err(StoreError::NotFound {
                    what: "group",
                    id: group.to_string(),
                });
            }
            let ids = tree.descendants(group);
            let groups: Vec<Group> = ids.iter().filter_map(|id| tree.get(*id)).cloned().collect();
            let hosts: Vec<Host> = Host::list(tx)?
                .into_iter()
                .filter(|h| h.group.is_some_and(|g| ids.contains(&g)))
                .collect();
            for host in &hosts {
                Host::delete(tx, host.id)?;
            }
            // Innermost first, so no row is left pointing at a deleted parent even briefly.
            for id in ids.iter().rev() {
                Group::delete(tx, *id)?;
            }
            let entry = TrashEntry::new(TrashContents { groups, hosts }, now);
            TrashEntry::upsert(tx, &entry)?;
            Ok(entry.id)
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::hosts::{Colour, GroupError, HostError, HostSettings};
    use ssherlock_core::{Compression, Port};

    use super::*;
    use crate::Repository;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn host(address: &str) -> Host {
        Host::new(address.parse().unwrap(), at(1))
    }

    fn hosts(store: &Store) -> Vec<Host> {
        let mut list: Vec<Host> = store.list().unwrap();
        list.sort_by_key(|h| h.id);
        list
    }

    fn groups(store: &Store) -> Vec<Group> {
        let mut list: Vec<Group> = store.list().unwrap();
        list.sort_by_key(|g| g.id);
        list
    }

    #[test]
    fn a_host_round_trips_with_every_field() {
        let store = Store::open_in_memory().unwrap();
        let group = Group::new("prod", None, at(1));
        store.upsert(&group).unwrap();
        let mut h = host("2001:db8::7");
        h.label = "Prod DB".into();
        h.group = Some(group.id);
        h.tags = Tag::parse_list("db, prod").unwrap();
        h.notes = "line one\nline two".into();
        h.is_favourite = true;
        h.settings = HostSettings {
            port: Port::new(2222),
            username: Some("postgres".into()),
            colour: Some(Colour::Red),
            compression: Some(Compression::On),
            ..HostSettings::default()
        };
        h.last_connected_at = Some(at(5));
        store.upsert(&h).unwrap();

        assert_eq!(store.get(h.id).unwrap(), Some(h.clone()));
        assert_eq!(hosts(&store), [h]);
        assert_eq!(
            Repository::<Host>::get(&store, Id::generate()).unwrap(),
            None
        );
    }

    #[test]
    fn saving_replaces_tags_and_keeps_the_last_connection() {
        let store = Store::open_in_memory().unwrap();
        let mut h = host("example.com");
        h.tags = Tag::parse_list("a b").unwrap();
        store.upsert(&h).unwrap();
        store.record_connection(h.id, at(50)).unwrap();

        // An editor holding a copy from before the connection saves it back.
        h.tags = Tag::parse_list("c").unwrap();
        h.label = "renamed".into();
        store.upsert(&h).unwrap();

        let saved: Host = store.get(h.id).unwrap().unwrap();
        assert_eq!(saved.tags, Tag::parse_list("c").unwrap());
        assert_eq!(saved.label, "renamed");
        assert_eq!(saved.last_connected_at, Some(at(50)));
        // Recording a connection for a deleted host is harmless.
        store.record_connection(Id::generate(), at(60)).unwrap();
    }

    #[test]
    fn invalid_hosts_and_groups_are_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut h = host("example.com");
        h.label = "a\nb".into();
        assert!(matches!(
            store.upsert(&h),
            Err(StoreError::InvalidHost(HostError::LabelControl))
        ));

        let outer = Group::new("outer", None, at(1));
        let inner = Group::new("inner", Some(outer.id), at(1));
        store.upsert_all(&[outer.clone(), inner.clone()]).unwrap();
        let looped = Group {
            parent: Some(inner.id),
            ..outer.clone()
        };
        assert!(matches!(
            store.upsert(&looped),
            Err(StoreError::InvalidGroup(GroupError::Cycle))
        ));
        let unnamed = Group::new(" ", None, at(1));
        assert!(matches!(
            store.upsert(&unnamed),
            Err(StoreError::InvalidGroup(GroupError::EmptyName))
        ));
        // A host in a group that doesn't exist breaks the foreign key.
        let mut orphan = host("example.org");
        orphan.group = Some(Id::generate());
        assert!(matches!(store.upsert(&orphan), Err(StoreError::Sqlite(_))));
    }

    #[test]
    fn upsert_all_is_all_or_nothing() {
        let store = Store::open_in_memory().unwrap();
        let good = host("a.example.com");
        let mut bad = host("b.example.com");
        bad.label = "x".repeat(500);
        assert!(store.upsert_all(&[good, bad]).is_err());
        assert_eq!(hosts(&store), Vec::new());
    }

    #[test]
    fn deleting_a_group_for_good_moves_its_contents_to_the_top() {
        let store = Store::open_in_memory().unwrap();
        let outer = Group::new("outer", None, at(1));
        let inner = Group::new("inner", Some(outer.id), at(1));
        store.upsert_all(&[outer.clone(), inner.clone()]).unwrap();
        let mut h = host("example.com");
        h.group = Some(outer.id);
        store.upsert(&h).unwrap();

        Repository::<Group>::delete(&store, outer.id).unwrap();
        assert_eq!(store.get(h.id).unwrap().map(|h: Host| h.group), Some(None));
        assert_eq!(
            store.get(inner.id).unwrap().map(|g: Group| g.parent),
            Some(None)
        );
    }

    #[test]
    fn trashing_hosts_makes_one_entry_each() {
        let store = Store::open_in_memory().unwrap();
        let a = host("a.example.com");
        let b = host("b.example.com");
        store.upsert_all(&[a.clone(), b.clone()]).unwrap();

        let entries = store.trash_hosts(&[a.id, b.id], at(9)).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(hosts(&store), Vec::new());
        let trash: Vec<TrashEntry> = store.list().unwrap();
        assert_eq!(trash.len(), 2);
        assert!(
            trash
                .iter()
                .all(|e| e.deleted_at == at(9) && e.contents.hosts.len() == 1)
        );

        // A missing host aborts the whole call.
        let c = host("c.example.com");
        store.upsert(&c).unwrap();
        assert!(matches!(
            store.trash_hosts(&[c.id, Id::generate()], at(10)),
            Err(StoreError::NotFound { what: "host", .. })
        ));
        assert_eq!(hosts(&store), [c]);
    }

    #[test]
    fn trashing_a_group_takes_its_subgroups_and_hosts() {
        let store = Store::open_in_memory().unwrap();
        let prod = Group::new("prod", None, at(1));
        let db = Group::new("db", Some(prod.id), at(1));
        let other = Group::new("other", None, at(1));
        store
            .upsert_all(&[prod.clone(), db.clone(), other.clone()])
            .unwrap();
        let mut in_db = host("db1");
        in_db.group = Some(db.id);
        let mut in_other = host("o1");
        in_other.group = Some(other.id);
        let loose = host("loose");
        store
            .upsert_all(&[in_db.clone(), in_other.clone(), loose.clone()])
            .unwrap();

        let entry_id = store.trash_group(prod.id, at(9)).unwrap();
        assert_eq!(groups(&store), [other]);
        let mut left = vec![in_other, loose];
        left.sort_by_key(|h| h.id);
        assert_eq!(hosts(&store), left);

        let entry: TrashEntry = store.get(entry_id).unwrap().unwrap();
        assert_eq!(entry.contents.groups, [prod, db]);
        assert_eq!(entry.contents.hosts, [in_db]);
        assert_eq!(entry.title(), "prod (1 host)");
        assert!(matches!(
            store.trash_group(Id::generate(), at(9)),
            Err(StoreError::NotFound { what: "group", .. })
        ));
    }

    /// The phase's exit check, at the store level: 50 hosts across nested groups.
    #[test]
    fn fifty_hosts_in_nested_groups() {
        let store = Store::open_in_memory().unwrap();
        let mut all_groups = Vec::new();
        let mut parent = None;
        for depth in 0..5 {
            let group = Group::new(format!("level {depth}"), parent, at(1));
            parent = Some(group.id);
            all_groups.push(group);
        }
        store.upsert_all(&all_groups).unwrap();
        let many: Vec<Host> = (0..50)
            .map(|i| {
                let mut h = host(&format!("10.0.0.{i}"));
                h.group = all_groups.get(i % all_groups.len()).map(|g| g.id);
                h.tags = Tag::parse_list(if i % 2 == 0 { "even" } else { "odd" }).unwrap();
                h
            })
            .collect();
        store.upsert_all(&many).unwrap();
        assert_eq!(hosts(&store).len(), 50);
        let tree_groups = groups(&store);
        let tree = GroupTree::new(&tree_groups);
        let deepest = all_groups.last().unwrap().id;
        assert_eq!(tree.ancestors(Some(deepest)).len(), 5);
    }
}
