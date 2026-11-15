//! Terminals on screen: the **view** layer of the terminal feature.
//!
//! [`TerminalView`] shows one terminal and forwards keyboard, mouse and paste input to it.
//! Emulation and I/O live in `ssherlock-terminal`; this module only paints snapshots and turns
//! gpui events into terminal input.

pub mod actions;
mod element;
mod keys;
pub mod view;

use gpui::{App, Global};
use ssherlock_terminal::ShellConfig;
pub use view::{TerminalView, TerminalViewEvent};

/// The shell new local terminals run. Absent means the user's login shell; settings (APP-02)
/// will set it, and tests set it so they never start the user's real shell.
#[derive(Clone, Debug, Default)]
pub struct DefaultShell(pub ShellConfig);

impl Global for DefaultShell {}

/// Sets up the terminal feature. Its actions are handled by each [`TerminalView`] and bound in
/// `keymap/default.json`, so there's no app-wide state to install yet.
pub fn init(_cx: &mut App) {}
