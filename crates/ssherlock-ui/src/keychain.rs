//! The keychain on screen: the **view** layer of the keychain feature (KEY-01…07, 13, 14).
//!
//! [`KeyList`] and [`IdentityList`] are the pages the hosts page shows when "Keys" or
//! "Identities" is chosen in its navigation column. [`KeyEditor`] generates, imports and edits
//! keys; [`IdentityEditor`] edits identities. Keys live in the shared
//! [`HostsModel`](crate::hosts::HostsModel), whose vault seals their secrets; key work itself
//! (generating, parsing, exporting) is `ssherlock_ssh::keys`.

pub mod actions;
pub mod detail;
pub mod editor;
pub mod identities;
pub mod identity_editor;
pub mod list;

pub use editor::{KeyEditor, KeyEditorEvent, KeyEditorMode};
use gpui::App;
pub use identities::IdentityList;
pub use identity_editor::{IdentityEditor, IdentityEditorEvent};
pub use list::KeyList;

/// Sets up the keychain feature. Its actions are handled by its views and bound in
/// `keymap/default.json`.
pub fn init(_cx: &mut App) {}
