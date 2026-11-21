//! Port-forwarding rules: their [`Repository`](crate::Repository) implementation, and saving
//! a host together with its rules.
//!
//! **Store** layer of the forwarding feature. Each [`Forward`] variant is spread over plain
//! columns (`kind`, `bind`, `port`, `target_host`, `target_port`), with `CHECK`s that only the
//! valid combinations can be stored.

use rusqlite::{Connection, OptionalExtension, Row, params};
use ssherlock_core::forwarding::{Bind, Destination, Forward, ForwardRule, Listen};
use ssherlock_core::hosts::Host;
use ssherlock_core::{Id, Port};

use crate::rows::{RowReader, Rows};
use crate::{Store, StoreError};

const COLUMNS: &str = "id, host_id, label, kind, bind, port, target_host, target_port, \
                       is_enabled, auto_start, created_at, updated_at";

/// A `forward_rules` row as stored, before parsing.
struct RuleRow {
    id: String,
    host_id: String,
    label: String,
    kind: String,
    bind: String,
    port: u16,
    target_host: Option<String>,
    target_port: Option<u16>,
    is_enabled: bool,
    auto_start: bool,
    created_at: String,
    updated_at: String,
}

impl RuleRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            host_id: row.get(1)?,
            label: row.get(2)?,
            kind: row.get(3)?,
            bind: row.get(4)?,
            port: row.get(5)?,
            target_host: row.get(6)?,
            target_port: row.get(7)?,
            is_enabled: row.get(8)?,
            auto_start: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }

    fn into_rule(self) -> Result<ForwardRule, StoreError> {
        let r = RowReader {
            table: "forward_rules",
            id: &self.id,
        };
        let corrupt = |column, reason: String| StoreError::Corrupt {
            table: "forward_rules",
            id: self.id.clone(),
            column,
            reason,
        };
        let bind = match self.bind.as_str() {
            "localhost" => Bind::Localhost,
            "all" => Bind::AllInterfaces,
            other => return Err(corrupt("bind", format!("{other:?} is not a bind address"))),
        };
        let listen = Listen {
            bind,
            port: r.parse("port", &self.port.to_string())?,
        };
        let destination = match (self.target_host.as_deref(), self.target_port) {
            (Some(host), Some(port)) => Some(Destination {
                host: r.parse("target_host", host)?,
                port: Port::try_from(port).map_err(|e| corrupt("target_port", e.to_string()))?,
            }),
            _ => None,
        };
        let forward = match (self.kind.as_str(), destination) {
            ("local", Some(destination)) => Forward::Local {
                listen,
                destination,
            },
            ("remote", Some(destination)) => Forward::Remote {
                listen,
                destination,
            },
            ("dynamic", None) => Forward::Dynamic { listen },
            (other, _) => {
                return Err(corrupt(
                    "kind",
                    format!("{other:?} does not match the target columns"),
                ));
            }
        };
        Ok(ForwardRule {
            id: r.id()?,
            host: r.parse("host_id", &self.host_id)?,
            label: self.label,
            forward,
            is_enabled: self.is_enabled,
            auto_start: self.auto_start,
            created_at: r.time("created_at", &self.created_at)?,
            updated_at: r.time("updated_at", &self.updated_at)?,
        })
    }
}

/// The `kind` column for `forward`.
fn kind(forward: &Forward) -> &'static str {
    match forward {
        Forward::Local { .. } => "local",
        Forward::Remote { .. } => "remote",
        Forward::Dynamic { .. } => "dynamic",
    }
}

/// The `bind` column for `bind`.
fn bind(bind: Bind) -> &'static str {
    match bind {
        Bind::Localhost => "localhost",
        Bind::AllInterfaces => "all",
    }
}

/// The rules of `host`, oldest first so a host's list keeps the order they were added in.
pub(crate) fn rules_of(conn: &Connection, host: Id<Host>) -> Result<Vec<ForwardRule>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM forward_rules WHERE host_id = ?1 ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([host.to_string()], RuleRow::read)?;
    rows.map(|row| row?.into_rule()).collect()
}

impl Rows for ForwardRule {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM forward_rules WHERE id = ?1"
        ))?
        .query_row([id.to_string()], RuleRow::read)
        .optional()?
        .map(RuleRow::into_rule)
        .transpose()
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM forward_rules ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], RuleRow::read)?;
        rows.map(|row| row?.into_rule()).collect()
    }

    fn upsert(conn: &Connection, rule: &Self) -> Result<(), StoreError> {
        rule.validate()?;
        let listen = rule.forward.listen();
        let destination = rule.forward.destination();
        conn.prepare_cached(&format!(
            "INSERT INTO forward_rules ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (id) DO UPDATE SET
                host_id = excluded.host_id,
                label = excluded.label,
                kind = excluded.kind,
                bind = excluded.bind,
                port = excluded.port,
                target_host = excluded.target_host,
                target_port = excluded.target_port,
                is_enabled = excluded.is_enabled,
                auto_start = excluded.auto_start,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at"
        ))?
        .execute(params![
            rule.id.to_string(),
            rule.host.to_string(),
            rule.label,
            kind(&rule.forward),
            bind(listen.bind),
            listen.port.get(),
            destination.map(|d| d.host.to_string()),
            destination.map(|d| d.port.get()),
            rule.is_enabled,
            rule.auto_start,
            rule.created_at.to_string(),
            rule.updated_at.to_string(),
        ])?;
        Ok(())
    }

    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        conn.prepare_cached("DELETE FROM forward_rules WHERE id = ?1")?
            .execute([id.to_string()])?;
        Ok(())
    }
}

impl Store {
    /// The rules saved with `host`, in the order they were added.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be read.
    pub fn forward_rules_of(&self, host: Id<Host>) -> Result<Vec<ForwardRule>, StoreError> {
        self.read(|conn| rules_of(conn, host))
    }

    /// Saves `host` and makes `rules` its port-forwarding rules, all or nothing: rules it had
    /// that aren't in `rules` are deleted. This is how the host editor saves.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidHost`] or [`StoreError::InvalidForwardRule`] if something fails
    /// validation (two enabled rules on one port, say), [`StoreError::ForeignForwardRule`] if a
    /// rule names another host, or a database error. Then nothing is saved.
    pub fn save_host_with_rules(
        &self,
        host: &Host,
        rules: &[ForwardRule],
    ) -> Result<(), StoreError> {
        if let Some(foreign) = rules.iter().find(|r| r.host != host.id) {
            return Err(StoreError::ForeignForwardRule {
                rule: foreign.id.to_string(),
            });
        }
        ForwardRule::validate_all(rules)?;
        self.write(|tx| {
            Host::upsert(tx, host)?;
            for old in rules_of(tx, host.id)? {
                if !rules.iter().any(|r| r.id == old.id) {
                    ForwardRule::delete(tx, old.id)?;
                }
            }
            rules
                .iter()
                .try_for_each(|rule| ForwardRule::upsert(tx, rule))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::Timestamp;
    use ssherlock_core::forwarding::ForwardRuleError;

    use super::*;
    use crate::Repository;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn port(n: u16) -> Port {
        Port::new(n).unwrap()
    }

    fn host(store: &Store) -> Host {
        let h = Host::new("example.com".parse().unwrap(), at(1));
        store.upsert(&h).unwrap();
        h
    }

    /// One rule of each kind and bind, so every column combination is written and read back.
    fn every_kind(host: Id<Host>) -> Vec<ForwardRule> {
        let destination = |h: &str, p| Destination {
            host: h.parse().unwrap(),
            port: port(p),
        };
        let mut local = ForwardRule::new(
            host,
            Forward::Local {
                listen: Listen::localhost(port(8080)),
                destination: destination("db.internal", 5432),
            },
            at(1),
        );
        local.label = "Postgres".into();
        local.auto_start = true;
        let mut remote = ForwardRule::new(
            host,
            Forward::Remote {
                listen: Listen {
                    bind: Bind::AllInterfaces,
                    port: port(9000),
                },
                destination: destination("::1", 3000),
            },
            at(2),
        );
        remote.is_enabled = false;
        let dynamic = ForwardRule::new(
            host,
            Forward::Dynamic {
                listen: Listen::localhost(port(1080)),
            },
            at(3),
        );
        vec![local, remote, dynamic]
    }

    #[test]
    fn every_kind_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let h = host(&store);
        let rules = every_kind(h.id);
        store.upsert_all(&rules).unwrap();
        assert_eq!(store.forward_rules_of(h.id).unwrap(), rules);
        assert_eq!(Repository::<ForwardRule>::list(&store).unwrap(), rules);
        let first = rules.first().unwrap();
        assert_eq!(store.get(first.id).unwrap().as_ref(), Some(first));
        Repository::<ForwardRule>::delete(&store, first.id).unwrap();
        assert_eq!(store.forward_rules_of(h.id).unwrap().len(), 2);
    }

    #[test]
    fn saving_a_host_replaces_its_rules() {
        let store = Store::open_in_memory().unwrap();
        let mut h = Host::new("new.example.com".parse().unwrap(), at(1));
        let mut rules = every_kind(h.id);
        // A new host and its rules arrive together.
        store.save_host_with_rules(&h, &rules).unwrap();
        assert_eq!(store.forward_rules_of(h.id).unwrap(), rules);

        h.label = "Renamed".into();
        let removed = rules.remove(1);
        if let Some(first) = rules.first_mut() {
            first.label = "Edited".into();
        }
        store.save_host_with_rules(&h, &rules).unwrap();
        assert_eq!(store.forward_rules_of(h.id).unwrap(), rules);
        assert_eq!(store.get(removed.id).unwrap(), None::<ForwardRule>);
        assert_eq!(
            store.get(h.id).unwrap().map(|h: Host| h.label),
            Some("Renamed".into())
        );
    }

    #[test]
    fn saving_bad_rules_saves_nothing() {
        let store = Store::open_in_memory().unwrap();
        let h = host(&store);
        let before = every_kind(h.id);
        store.save_host_with_rules(&h, &before).unwrap();

        // The disabled remote rule enabled, and a second local rule on 8080.
        let mut clashing = before.clone();
        let clash = ForwardRule::new(
            h.id,
            Forward::Dynamic {
                listen: Listen::localhost(port(8080)),
            },
            at(4),
        );
        clashing.push(clash);
        let renamed = Host {
            label: "not saved".into(),
            ..h.clone()
        };
        assert!(matches!(
            store.save_host_with_rules(&renamed, &clashing),
            Err(StoreError::InvalidForwardRule(
                ForwardRuleError::Clash { .. }
            ))
        ));
        let other_host = ForwardRule::new(
            Id::generate(),
            Forward::Dynamic {
                listen: Listen::localhost(port(1)),
            },
            at(4),
        );
        assert!(matches!(
            store.save_host_with_rules(&renamed, &[other_host]),
            Err(StoreError::ForeignForwardRule { .. })
        ));
        assert_eq!(store.forward_rules_of(h.id).unwrap(), before);
        assert_eq!(store.get(h.id).unwrap(), Some(h));
    }

    #[test]
    fn rules_go_with_a_host_deleted_for_good() {
        let store = Store::open_in_memory().unwrap();
        let h = host(&store);
        store.upsert_all(&every_kind(h.id)).unwrap();
        Repository::<Host>::delete(&store, h.id).unwrap();
        assert_eq!(Repository::<ForwardRule>::list(&store).unwrap(), Vec::new());
    }

    #[test]
    fn a_rule_needs_its_host() {
        let store = Store::open_in_memory().unwrap();
        let orphan = every_kind(Id::generate()).remove(0);
        assert!(matches!(store.upsert(&orphan), Err(StoreError::Sqlite(_))));
    }

    #[test]
    fn impossible_rows_are_refused_by_the_schema() {
        let store = Store::open_in_memory().unwrap();
        let h = host(&store);
        let insert = |kind: &str, target: Option<&str>, port: i64| {
            store.write(|tx| {
                tx.execute(
                    "INSERT INTO forward_rules (id, host_id, kind, bind, port, target_host,
                        target_port, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'localhost', ?4, ?5, ?6, 't', 't')",
                    params![
                        Id::<ForwardRule>::generate().to_string(),
                        h.id.to_string(),
                        kind,
                        port,
                        target,
                        target.map(|_| 80),
                    ],
                )?;
                Ok(())
            })
        };
        assert!(insert("local", Some("db"), 8080).is_ok());
        assert!(insert("local", None, 8080).is_err());
        assert!(insert("dynamic", Some("db"), 8080).is_err());
        assert!(insert("sideways", None, 8080).is_err());
        assert!(insert("dynamic", None, 0).is_err());
        assert!(insert("dynamic", None, 65_536).is_err());
    }
}
