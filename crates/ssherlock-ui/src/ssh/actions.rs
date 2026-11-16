//! SSH actions. `QuickConnect` is handled by the pane (key context `Pane`); `ExportKeyHere` by
//! a remote terminal (key context `RemoteTerminal`).

use gpui::actions;

actions!(
    ssh,
    [
        /// Opens the quick connect bar to type `user@host:port` and connect.
        QuickConnect,
        /// Adds a keychain key to this server's `authorized_keys`, over the tab's connection.
        ExportKeyHere,
    ]
);
