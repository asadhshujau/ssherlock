//! Keys and identities: their [`Repository`](crate::Repository) implementations.
//!
//! **Store** layer of the keychain feature. Private keys, passphrases and passwords arrive
//! already sealed by the vault and are stored as BLOBs. Deleting a key or identity also clears
//! every host and group setting that names it, in the same transaction.

use rusqlite::{Connection, OptionalExtension, Row, params};
use ssherlock_core::keychain::{Identity, Key, KeyProtection};
use ssherlock_core::{Id, Sealed};

use crate::StoreError;
use crate::rows::{RowReader, Rows};

const KEY_COLUMNS: &str = "id, label, kind, public_key, fingerprint, private_key, protection, \
                           passphrase, created_at, updated_at";

/// A `keys` row as stored, before parsing.
struct KeyRow {
    id: String,
    label: String,
    kind: String,
    public_key: String,
    fingerprint: String,
    private_key: Vec<u8>,
    protection: String,
    passphrase: Option<Vec<u8>>,
    created_at: String,
    updated_at: String,
}

impl KeyRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            label: row.get(1)?,
            kind: row.get(2)?,
            public_key: row.get(3)?,
            fingerprint: row.get(4)?,
            private_key: row.get(5)?,
            protection: row.get(6)?,
            passphrase: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }

    fn into_key(self) -> Result<Key, StoreError> {
        let r = RowReader {
            table: "keys",
            id: &self.id,
        };
        let protection = match (self.protection.as_str(), self.passphrase) {
            ("unencrypted", None) => KeyProtection::Unencrypted,
            ("ask", None) => KeyProtection::AskPassphrase,
            ("saved", Some(sealed)) => KeyProtection::SavedPassphrase(Sealed::from_bytes(sealed)),
            (other, _) => {
                return Err(StoreError::Corrupt {
                    table: "keys",
                    id: self.id.clone(),
                    column: "protection",
                    reason: format!("{other:?} does not match the passphrase column"),
                });
            }
        };
        Ok(Key {
            id: r.id()?,
            label: self.label,
            kind: r.parse("kind", &self.kind)?,
            public_key: self.public_key,
            fingerprint: self.fingerprint,
            private_key: Sealed::from_bytes(self.private_key),
            protection,
            created_at: r.time("created_at", &self.created_at)?,
            updated_at: r.time("updated_at", &self.updated_at)?,
        })
    }
}

/// `protection` and `passphrase` column values for `protection`.
fn protection_columns(protection: &KeyProtection) -> (&'static str, Option<&[u8]>) {
    match protection {
        KeyProtection::Unencrypted => ("unencrypted", None),
        KeyProtection::AskPassphrase => ("ask", None),
        KeyProtection::SavedPassphrase(sealed) => ("saved", Some(sealed.as_bytes())),
    }
}

/// Removes `field` from the JSON settings of every host and group where it equals `id`.
fn clear_setting(conn: &Connection, field: &str, id: &str) -> Result<(), StoreError> {
    for table in ["hosts", "groups"] {
        conn.execute(
            &format!(
                "UPDATE {table} SET settings = json_remove(settings, '$.{field}')
                 WHERE json_extract(settings, '$.{field}') = ?1"
            ),
            [id],
        )?;
    }
    Ok(())
}

impl Rows for Key {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        conn.prepare_cached(&format!("SELECT {KEY_COLUMNS} FROM keys WHERE id = ?1"))?
            .query_row([id.to_string()], KeyRow::read)
            .optional()?
            .map(KeyRow::into_key)
            .transpose()
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut stmt = conn.prepare_cached(&format!("SELECT {KEY_COLUMNS} FROM keys"))?;
        let rows = stmt.query_map([], KeyRow::read)?;
        rows.map(|row| row?.into_key()).collect()
    }

    fn upsert(conn: &Connection, key: &Self) -> Result<(), StoreError> {
        key.validate()?;
        let (protection, passphrase) = protection_columns(&key.protection);
        conn.prepare_cached(&format!(
            "INSERT INTO keys ({KEY_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (id) DO UPDATE SET
                label = excluded.label,
                kind = excluded.kind,
                public_key = excluded.public_key,
                fingerprint = excluded.fingerprint,
                private_key = excluded.private_key,
                protection = excluded.protection,
                passphrase = excluded.passphrase,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at"
        ))?
        .execute(params![
            key.id.to_string(),
            key.label,
            key.kind.to_string(),
            key.public_key,
            key.fingerprint,
            key.private_key.as_bytes(),
            protection,
            passphrase,
            key.created_at.to_string(),
            key.updated_at.to_string(),
        ])?;
        Ok(())
    }

    /// Also stops every host and group from naming the key; identities let go of it through
    /// their foreign key.
    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        let id = id.to_string();
        clear_setting(conn, "key", &id)?;
        conn.prepare_cached("DELETE FROM keys WHERE id = ?1")?
            .execute([&id])?;
        Ok(())
    }
}

const IDENTITY_COLUMNS: &str = "id, label, username, password, key_id, created_at, updated_at";

/// An `identities` row as stored, before parsing.
struct IdentityRow {
    id: String,
    label: String,
    username: String,
    password: Option<Vec<u8>>,
    key_id: Option<String>,
    created_at: String,
    updated_at: String,
}

impl IdentityRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            label: row.get(1)?,
            username: row.get(2)?,
            password: row.get(3)?,
            key_id: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    fn into_identity(self) -> Result<Identity, StoreError> {
        let r = RowReader {
            table: "identities",
            id: &self.id,
        };
        Ok(Identity {
            id: r.id()?,
            label: self.label,
            username: self.username,
            password: self.password.map(Sealed::from_bytes),
            key: r.parse_opt("key_id", self.key_id.as_deref())?,
            created_at: r.time("created_at", &self.created_at)?,
            updated_at: r.time("updated_at", &self.updated_at)?,
        })
    }
}

impl Rows for Identity {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError> {
        conn.prepare_cached(&format!(
            "SELECT {IDENTITY_COLUMNS} FROM identities WHERE id = ?1"
        ))?
        .query_row([id.to_string()], IdentityRow::read)
        .optional()?
        .map(IdentityRow::into_identity)
        .transpose()
    }

    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError> {
        let mut stmt =
            conn.prepare_cached(&format!("SELECT {IDENTITY_COLUMNS} FROM identities"))?;
        let rows = stmt.query_map([], IdentityRow::read)?;
        rows.map(|row| row?.into_identity()).collect()
    }

    fn upsert(conn: &Connection, identity: &Self) -> Result<(), StoreError> {
        identity.validate()?;
        conn.prepare_cached(&format!(
            "INSERT INTO identities ({IDENTITY_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET
                label = excluded.label,
                username = excluded.username,
                password = excluded.password,
                key_id = excluded.key_id,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at"
        ))?
        .execute(params![
            identity.id.to_string(),
            identity.label,
            identity.username,
            identity.password.as_ref().map(Sealed::as_bytes),
            identity.key.map(|k| k.to_string()),
            identity.created_at.to_string(),
            identity.updated_at.to_string(),
        ])?;
        Ok(())
    }

    /// Also stops every host and group from picking the identity.
    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError> {
        let id = id.to_string();
        clear_setting(conn, "identity", &id)?;
        conn.prepare_cached("DELETE FROM identities WHERE id = ?1")?
            .execute([&id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::Timestamp;
    use ssherlock_core::hosts::{Group, Host};
    use ssherlock_core::keychain::{EcdsaCurve, KeyError, KeyKind};

    use super::*;
    use crate::{Repository, Store};

    fn key(protection: KeyProtection) -> Key {
        Key {
            id: Id::generate(),
            label: "Laptop".into(),
            kind: KeyKind::Ecdsa(EcdsaCurve::P384),
            public_key: "ecdsa-sha2-nistp384 AAAA me@laptop".into(),
            fingerprint: "SHA256:xyz".into(),
            private_key: Sealed::from_bytes(vec![1, 2, 3, 4]),
            protection,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::from_second(9).unwrap(),
        }
    }

    #[test]
    fn keys_round_trip_with_each_protection() {
        let store = Store::open_in_memory().unwrap();
        for protection in [
            KeyProtection::Unencrypted,
            KeyProtection::AskPassphrase,
            KeyProtection::SavedPassphrase(Sealed::from_bytes(vec![7; 8])),
        ] {
            let k = key(protection);
            store.upsert(&k).unwrap();
            assert_eq!(store.get(k.id).unwrap(), Some(k.clone()));
            // Saving again replaces, including a changed protection.
            let mut edited = k.clone();
            edited.label = "Renamed".into();
            edited.protection = KeyProtection::Unencrypted;
            store.upsert(&edited).unwrap();
            assert_eq!(store.get(k.id).unwrap(), Some(edited));
        }
        assert_eq!(Repository::<Key>::list(&store).unwrap().len(), 3);
    }

    #[test]
    fn an_invalid_key_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let mut k = key(KeyProtection::Unencrypted);
        k.public_key = "garbage".into();
        assert!(matches!(
            store.upsert(&k),
            Err(StoreError::InvalidKey(KeyError::PublicKeyShape))
        ));
    }

    #[test]
    fn identities_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let k = key(KeyProtection::Unencrypted);
        store.upsert(&k).unwrap();
        let mut identity = Identity::new("deploy", Timestamp::UNIX_EPOCH);
        identity.label = "Deploy".into();
        identity.password = Some(Sealed::from_bytes(vec![5; 3]));
        identity.key = Some(k.id);
        store.upsert(&identity).unwrap();
        assert_eq!(store.get(identity.id).unwrap(), Some(identity.clone()));
        assert_eq!(Repository::<Identity>::list(&store).unwrap(), [identity]);
    }

    #[test]
    fn deleting_a_key_or_identity_clears_everything_that_names_it() {
        let store = Store::open_in_memory().unwrap();
        let now = Timestamp::UNIX_EPOCH;
        let k = key(KeyProtection::Unencrypted);
        let other = key(KeyProtection::Unencrypted);
        store.upsert_all(&[k.clone(), other.clone()]).unwrap();
        let mut identity = Identity::new("deploy", now);
        identity.key = Some(k.id);
        store.upsert(&identity).unwrap();

        let mut group = Group::new("prod", None, now);
        group.settings.key = Some(k.id);
        group.settings.identity = Some(identity.id);
        store.upsert(&group).unwrap();
        let mut host = Host::new("example.com".parse().unwrap(), now);
        host.settings.key = Some(k.id);
        host.settings.identity = Some(identity.id);
        host.settings.username = Some("root".into());
        let mut bystander = Host::new("other.example.com".parse().unwrap(), now);
        bystander.settings.key = Some(other.id);
        store
            .upsert_all(&[host.clone(), bystander.clone()])
            .unwrap();

        Repository::<Key>::delete(&store, k.id).unwrap();
        let host_after: Host = store.get(host.id).unwrap().unwrap();
        let group_after: Group = store.get(group.id).unwrap().unwrap();
        let identity_after: Identity = store.get(identity.id).unwrap().unwrap();
        assert_eq!(host_after.settings.key, None);
        assert_eq!(host_after.settings.identity, Some(identity.id));
        assert_eq!(host_after.settings.username.as_deref(), Some("root"));
        assert_eq!(group_after.settings.key, None);
        assert_eq!(identity_after.key, None);
        assert_eq!(store.get(bystander.id).unwrap(), Some(bystander));

        Repository::<Identity>::delete(&store, identity.id).unwrap();
        let host_after: Host = store.get(host.id).unwrap().unwrap();
        let group_after: Group = store.get(group.id).unwrap().unwrap();
        assert_eq!(host_after.settings.identity, None);
        assert_eq!(group_after.settings.identity, None);
        assert_eq!(
            Repository::<Identity>::get(&store, identity.id).unwrap(),
            None
        );
    }
}
