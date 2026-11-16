//! The keychain on screen: the **view** layer of the keychain feature (KEY-01…07, 13, 14).
//!
//! [`KeyList`] is the keys page the hosts page shows when "Keys" is chosen in its navigation
//! column. [`KeyEditor`] generates, imports and edits keys. Keys live in the shared
//! [`HostsModel`](crate::hosts::HostsModel), whose vault seals their secrets; key work itself
//! (generating, parsing, exporting) is `ssherlock_ssh::keys`.

pub mod actions;
pub mod detail;
pub mod editor;
pub mod list;

pub use editor::{KeyEditor, KeyEditorEvent, KeyEditorMode};
use gpui::App;
pub use list::KeyList;

/// Sets up the keychain feature. Its actions are handled by its views and bound in
/// `keymap/default.json`.
pub fn init(_cx: &mut App) {}
