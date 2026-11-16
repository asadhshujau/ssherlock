//! Domain types, ids, errors and traits shared by every SSHerlock crate.
//!
//! This is the **model** layer of each feature slice. It depends on no other workspace crate
//! and does no I/O, so everything here is pure and unit-testable.
//!
//! Errors follow one convention: each model module defines its own `<Area>Error` enum with
//! `thiserror`, whose messages are lower-case sentences without a trailing period that name the
//! thing that failed. Nothing here wraps another workspace crate's error.
#![forbid(unsafe_code)]

pub mod address;
pub mod algorithms;
pub mod app;
pub mod hosts;
pub mod id;
pub mod port;
pub mod quick_connect;
pub mod secret;
pub mod time;

pub use address::{Address, AddressError};
pub use algorithms::{AlgorithmPreferences, Compression};
pub use id::{Id, IdError};
pub use port::{Port, PortError};
pub use quick_connect::{ConnectTarget, QuickConnectError};
pub use secret::{ExposeSecret, SecretBox, SecretString};
pub use time::Timestamp;
