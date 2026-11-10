//! Domain types, ids, errors and traits shared by every SSHerlock crate.
//!
//! This is the **model** layer of each feature slice. It depends on no other workspace crate
//! and does no I/O, so everything here is pure and unit-testable.
#![forbid(unsafe_code)]
