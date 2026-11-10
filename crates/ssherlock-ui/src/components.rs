//! Generic widgets that features are assembled from (CLAUDE.md §7). Nothing here knows about
//! terminals, hosts or any other feature.

pub mod tab_bar;
pub mod text_input;

pub use tab_bar::{Tab, TabBar};
pub use text_input::{InputKind, TextInput, TextInputEvent};
