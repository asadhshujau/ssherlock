//! Terminal emulation: the VT grid, local ptys and terminal events.
//!
//! This is a **service** crate. It turns input bytes into grid snapshots and events, and knows
//! nothing about how they are drawn.
#![forbid(unsafe_code)]
