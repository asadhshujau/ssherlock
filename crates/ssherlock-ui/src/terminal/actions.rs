//! Actions a terminal handles while it or its search bar is focused (key contexts `Terminal`
//! and `TerminalSearch`).

use gpui::actions;

actions!(
    terminal,
    [
        /// Copies the selected text to the clipboard.
        Copy,
        /// Pastes the clipboard into the terminal, bracketed if the program asked for it.
        Paste,
        /// Scrolls the view one line up into the scrollback.
        ScrollLineUp,
        /// Scrolls the view one line down.
        ScrollLineDown,
        /// Scrolls the view one screen up into the scrollback.
        ScrollPageUp,
        /// Scrolls the view one screen down.
        ScrollPageDown,
        /// Scrolls to the oldest line of the scrollback.
        ScrollToTop,
        /// Scrolls back to the live screen.
        ScrollToBottom,
        /// Deletes the scrollback, keeping the screen.
        ClearScrollback,
        /// Resets the terminal as if newly started, for when a program left it garbled.
        ResetTerminal,
        /// Opens the search bar, or focuses it if already open.
        Find,
        /// Goes to the previous (older) search match.
        SearchOlder,
        /// Goes to the next (newer) search match.
        SearchNewer,
        /// Closes the search bar and clears the highlights.
        DismissSearch,
        /// Makes terminal text one step larger, in every terminal.
        ZoomIn,
        /// Makes terminal text one step smaller, in every terminal.
        ZoomOut,
        /// Returns terminal text to the theme's size.
        ResetZoom,
    ]
);
