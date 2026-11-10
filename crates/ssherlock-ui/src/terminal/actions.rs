//! Actions a terminal handles while focused (key context `Terminal`).

use gpui::actions;

actions!(
    terminal,
    [
        /// Pastes the clipboard into the terminal, bracketed if the program asked for it.
        Paste,
    ]
);
