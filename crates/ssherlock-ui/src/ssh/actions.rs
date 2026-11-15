//! SSH actions. `QuickConnect` is handled by the pane (key context `Pane`).

use gpui::actions;

actions!(
    ssh,
    [
        /// Opens the quick connect bar to type `user@host:port` and connect.
        QuickConnect,
    ]
);
