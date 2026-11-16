//! Saved hosts on screen: the **view** layer of the hosts feature.
//!
//! [`HostList`] is the hosts page the pane shows behind its "Hosts" tab. [`HostsModel`] is the
//! one in-memory copy of the saved hosts that every view reads, backed by `ssherlock-store`.
//! The selection rules ([`selection`]) and the navigation column ([`nav`]) are plain data,
//! tested without gpui.

pub mod actions;
pub mod detail;
pub mod item;
pub mod list;
pub mod model;
pub mod nav;
pub mod selection;

use gpui::App;
pub use list::{ConnectRequest, HostList, HostListEvent};
pub use model::{HostsModel, LoadState};

/// Sets up the hosts feature. Its actions are handled by the hosts page and bound in
/// `keymap/default.json`; the model is installed by `main` once it knows where the database
/// lives ([`HostsModel::open`]).
pub fn init(_cx: &mut App) {}
