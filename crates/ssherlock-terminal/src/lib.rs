//! Terminal emulation: the VT grid, local ptys and terminal events.
//!
//! This is a **service** crate. It turns input bytes into grid snapshots and events, and knows
//! nothing about how they are drawn. Its public API uses only its own types, so the UI never
//! depends on the emulator underneath (`alacritty_terminal`).
#![forbid(unsafe_code)]

pub mod palette;
pub mod size;
pub mod snapshot;
pub mod terminal;

pub use palette::{Palette, Rgb};
pub use size::TerminalSize;
pub use snapshot::{
    CellStyle, Cursor, CursorShape, MouseEncoding, MouseReporting, Snapshot, SnapshotCell,
    TerminalModes, Underline,
};
pub use terminal::{Terminal, TerminalConfig, TerminalEvent};
