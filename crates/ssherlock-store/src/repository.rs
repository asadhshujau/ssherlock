//! The four operations every persisted model supports.

use ssherlock_core::Id;

use crate::StoreError;

/// Loading and saving one kind of record. Implemented by [`Store`](crate::Store) once per
/// model, so every feature reads and writes its data the same way.
pub trait Repository<T> {
    /// The record with `id`, or `None` if there isn't one.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be read.
    fn get(&self, id: Id<T>) -> Result<Option<T>, StoreError>;

    /// Every record, in no particular order.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be read.
    fn list(&self) -> Result<Vec<T>, StoreError>;

    /// Inserts `item`, or replaces the record with its id.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if `item` is invalid or the database can't be written.
    fn upsert(&self, item: &T) -> Result<(), StoreError>;

    /// Like [`upsert`](Self::upsert) for each item, all or nothing (bulk edits, HOST-08).
    ///
    /// # Errors
    ///
    /// A [`StoreError`] for the first item that fails; then nothing is saved.
    fn upsert_all(&self, items: &[T]) -> Result<(), StoreError>;

    /// Deletes the record with `id` for good. Doing nothing if there isn't one is not an error.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the database can't be written.
    fn delete(&self, id: Id<T>) -> Result<(), StoreError>;
}
