//! Port forwarding on screen: the **view** layer of the forwarding feature (FWD-01…08,
//! HOST-11).
//!
//! [`RuleEditor`] is the form for one rule; the host editor opens it to add and change a
//! host's rules. [`PortAuthority`] is the Port forwarding page, where rules are started and
//! stopped and their traffic shows. [`ForwardsModel`] holds what's running. [`item`] draws a
//! rule as a row. Running forwards is `ssherlock-ssh`'s job; saving rules goes through
//! [`HostsModel`](crate::hosts::HostsModel).

pub mod actions;
pub mod editor;
pub mod item;
pub mod list;
pub mod model;
#[cfg(test)]
mod test_support;

pub use editor::{RuleEditor, RuleEditorEvent};
use gpui::App;
pub use list::{PortAuthority, PortAuthorityEvent};
pub use model::ForwardsModel;

/// Sets up the forwarding feature: the app's [`ForwardsModel`]. Its actions are handled by the
/// views that show rules and bound in `keymap/default.json`.
pub fn init(cx: &mut App) {
    ForwardsModel::install(cx);
}
