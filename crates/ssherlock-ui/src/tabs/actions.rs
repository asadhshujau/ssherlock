//! Actions a pane handles while it or anything inside it is focused (key context `Pane`).

use gpui::actions;

actions!(
    pane,
    [
        /// Opens a new terminal tab after the active one.
        NewTab,
        /// Closes the active tab.
        CloseTab,
        /// Activates the tab to the right, wrapping around.
        NextTab,
        /// Activates the tab to the left, wrapping around.
        PreviousTab,
        /// Moves the active tab one place left.
        MoveTabLeft,
        /// Moves the active tab one place right.
        MoveTabRight,
        /// Edits the active tab's name.
        RenameTab,
    ]
);
