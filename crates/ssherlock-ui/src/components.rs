//! Generic widgets that features are assembled from (CLAUDE.md §7). Nothing here knows about
//! terminals, hosts or any other feature.

pub mod button;
pub mod checkbox;
pub mod context_menu;
pub mod form;
pub mod modal;
pub mod picker;
pub mod segmented;
pub mod tab_bar;
pub mod text_input;

pub use button::{Button, ButtonStyle};
pub use checkbox::Checkbox;
pub use context_menu::{ContextMenu, MenuDismissed, MenuItem};
pub use form::{Form, FormField, FormSection};
pub use modal::{Modal, ModalEvent};
pub use picker::{Picker, PickerEvent, PickerItem};
pub use segmented::{Segment, SegmentedControl};
pub use tab_bar::{Tab, TabBar};
pub use text_input::{InputKind, TextInput, TextInputEvent};
