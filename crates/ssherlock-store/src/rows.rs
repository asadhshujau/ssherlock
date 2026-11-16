//! Shared plumbing for repositories: the per-model row functions, the one generic
//! [`Repository`] implementation built on them, and helpers for reading stored text back.

use std::fmt::Display;
use std::str::FromStr;

use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ssherlock_core::{Id, Timestamp};

use crate::{Repository, Store, StoreError};

/// How one model is read and written, given a connection (or a transaction, which derefs to
/// one). Each model implements this once; [`Repository`] comes from it, and feature code reuses
/// it inside larger transactions such as restoring from the trash.
pub(crate) trait Rows: Sized {
    fn get(conn: &Connection, id: Id<Self>) -> Result<Option<Self>, StoreError>;
    fn list(conn: &Connection) -> Result<Vec<Self>, StoreError>;
    fn upsert(conn: &Connection, item: &Self) -> Result<(), StoreError>;
    fn delete(conn: &Connection, id: Id<Self>) -> Result<(), StoreError>;
}

impl<T: Rows> Repository<T> for Store {
    fn get(&self, id: Id<T>) -> Result<Option<T>, StoreError> {
        self.read(|conn| T::get(conn, id))
    }

    fn list(&self) -> Result<Vec<T>, StoreError> {
        self.read(T::list)
    }

    fn upsert(&self, item: &T) -> Result<(), StoreError> {
        self.write(|tx| T::upsert(tx, item))
    }

    fn upsert_all(&self, items: &[T]) -> Result<(), StoreError> {
        self.write(|tx| items.iter().try_for_each(|item| T::upsert(tx, item)))
    }

    fn delete(&self, id: Id<T>) -> Result<(), StoreError> {
        self.write(|tx| T::delete(tx, id))
    }
}

/// Reads columns of one stored row back into model types, naming the row in any error.
pub(crate) struct RowReader<'a> {
    pub table: &'static str,
    pub id: &'a str,
}

impl RowReader<'_> {
    fn corrupt(&self, column: &'static str, reason: impl Display) -> StoreError {
        StoreError::Corrupt {
            table: self.table,
            id: self.id.to_owned(),
            column,
            reason: reason.to_string(),
        }
    }

    /// The row's own id.
    pub fn id<T>(&self) -> Result<Id<T>, StoreError> {
        self.parse("id", self.id)
    }

    /// Any column whose type parses from text: ids, addresses, timestamps.
    pub fn parse<T: FromStr>(&self, column: &'static str, text: &str) -> Result<T, StoreError>
    where
        T::Err: Display,
    {
        text.parse().map_err(|error| self.corrupt(column, error))
    }

    /// An optional column.
    pub fn parse_opt<T: FromStr>(
        &self,
        column: &'static str,
        text: Option<&str>,
    ) -> Result<Option<T>, StoreError>
    where
        T::Err: Display,
    {
        text.map(|text| self.parse(column, text)).transpose()
    }

    /// A timestamp column.
    pub fn time(&self, column: &'static str, text: &str) -> Result<Timestamp, StoreError> {
        self.parse(column, text)
    }

    /// A JSON column.
    pub fn json<T: DeserializeOwned>(
        &self,
        column: &'static str,
        text: &str,
    ) -> Result<T, StoreError> {
        serde_json::from_str(text).map_err(|error| self.corrupt(column, error))
    }
}

/// `value` as JSON text for a column.
pub(crate) fn to_json(what: &'static str, value: &impl Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|source| StoreError::Encode { what, source })
}
