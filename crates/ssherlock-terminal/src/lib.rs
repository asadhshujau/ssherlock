//! Terminal emulation: the VT grid, local ptys and terminal events.
//!
//! This is a **service** crate. It turns program output into grid snapshots and events, and
//! knows nothing about how they are drawn. Its public API uses only its own types, so the UI
//! never depends on the emulator underneath (`alacritty_terminal`).
//!
//! - [`Terminal`] is the emulator itself: pure, synchronous, no I/O.
//! - [`pty`] connects it to a program: [`spawn_local`] starts a local shell, and [`PtyIo`] is
//!   the channel pair any other byte source (an SSH channel) provides.
//! - [`input`] encodes keys, paste, focus and mouse as the bytes programs expect.
//! - [`TerminalHandle`] runs a `Terminal` against a `PtyIo` on Tokio; this is what the UI holds.
#![forbid(unsafe_code)]

pub mod error;
pub mod handle;
pub mod input;
pub mod link;
pub mod palette;
pub mod pty;
pub mod search;
pub mod selection;
pub mod size;
pub mod snapshot;
pub mod terminal;

pub use error::TerminalError;
pub use handle::TerminalHandle;
pub use input::{
    Key, Modifiers, MouseAction, MouseButton, MouseInput, encode_alternate_scroll, encode_focus,
    encode_key, encode_mouse, encode_paste, is_multiline,
};
pub use link::Link;
pub use palette::{Palette, Rgb};
pub use pty::{
    PtyExit, PtyInput, PtyIo, PtyOutput, PtyPeer, ShellConfig, ShellProgram, spawn_local,
};
pub use search::{SearchDirection, SearchStatus};
pub use selection::{CellSide, GridPoint, SelectionKind, VisibleRange};
pub use size::TerminalSize;
pub use snapshot::{
    CellStyle, Cursor, CursorShape, MouseEncoding, MouseReporting, Snapshot, SnapshotCell,
    TerminalModes, Underline,
};
pub use terminal::{ScrollAmount, Terminal, TerminalConfig, TerminalEvent};
