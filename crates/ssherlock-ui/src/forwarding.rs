//! Port forwarding on screen: the **view** layer of the forwarding feature (FWD-01…08,
//! HOST-11).
//!
//! [`RuleEditor`] is the form for one rule; the host editor opens it to add and change a
//! host's rules. [`item`] draws a rule as a row. Running forwards is `ssherlock-ssh`'s job;
//! saving rules goes through [`HostsModel`](crate::hosts::HostsModel).

pub mod actions;
pub mod editor;
pub mod item;

pub use editor::{RuleEditor, RuleEditorEvent};
use gpui::App;

/// Sets up the forwarding feature. Its actions are handled by the views that show rules and
/// bound in `keymap/default.json`.
pub fn init(_cx: &mut App) {}
