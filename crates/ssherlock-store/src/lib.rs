//! SQLite persistence, migrations and vault encryption.
//!
//! This is the **store** layer of each feature slice. [`Store`] owns the database; each
//! persisted model gets one [`Repository`] implementation with the same four operations, plus
//! whatever feature-specific queries it needs (moving hosts to the trash, recording a
//! connection). Every call is blocking and quick; the UI runs them on a background thread.
#![forbid(unsafe_code)]

mod error;
mod hosts;
mod migrations;
mod repository;
mod rows;
mod store;
mod trash;

pub use error::StoreError;
pub use repository::Repository;
pub use store::Store;
