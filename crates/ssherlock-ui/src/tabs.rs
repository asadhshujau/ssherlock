//! Tabs of terminals: the **view** layer of the tabs feature.
//!
//! A [`Pane`] holds a row of tabs, each showing one terminal. The tab order and the active tab
//! are plain data in [`list::TabList`], tested without gpui.

pub mod actions;
pub mod list;
pub mod pane;

use gpui::App;
pub use pane::Pane;

/// Sets up the tabs feature. Its actions are handled by each [`Pane`] and bound in
/// `keymap/default.json`, so there's no app-wide state to install yet.
pub fn init(_cx: &mut App) {}
