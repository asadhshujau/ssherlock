//! Domain types, ids, errors and traits shared by every SSHerlock crate.
//!
//! This is the **model** layer of each feature slice. It depends on no other workspace crate
//! and does no I/O, so everything here is pure and unit-testable.
//!
//! Errors follow one convention: each model module defines its own `<Area>Error` enum with
//! `thiserror`, whose messages are lower-case sentences without a trailing period that name the
//! thing that failed. Nothing here wraps another workspace crate's error.
#![forbid(unsafe_code)]

pub mod app;
pub mod id;
pub mod secret;

pub use id::{Id, IdError};
pub use secret::{ExposeSecret, SecretBox, SecretString};
