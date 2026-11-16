//! Saved hosts, the groups they're organised in, and their tags (HOST-01…09).
//!
//! **Model** layer of the hosts feature. Everything here is pure: [`Host`] and [`Group`] are
//! plain records, [`GroupTree`] answers questions about nesting, [`resolve`] works out the
//! settings a host ends up with after inheriting from its groups, and [`search`] filters, sorts
//! and ranks hosts for the host list and the quick connect bar.

pub mod bulk;
mod colour;
mod group;
mod host;
mod resolve;
pub mod search;
mod settings;
mod tag;

pub use bulk::{BulkEdit, Change};
pub use colour::Colour;
pub use group::{Group, GroupError, GroupRow, GroupTree};
pub use host::{Host, HostError};
pub use resolve::{EffectiveHost, EffectiveSettings, Resolved, Source, resolve};
pub use search::{HostQuery, Scope, SortOrder};
pub use settings::{HostSettings, SettingsError};
pub use tag::{Tag, TagError};

/// The longest a host label or group name may be, in characters. Long enough for any sensible
/// name, short enough that a list row can show it.
pub const MAX_NAME_LEN: usize = 120;

/// Whether `text` holds a character that would break a one-line label (a newline, tab or other
/// control character).
fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}
