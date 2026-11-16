//! Actions of the hosts page. Each is bound in `keymap/default.json` under the key context of
//! the part of the page it acts on: `HostNav`, `HostList`, `HostSearch`, `Trash` or
//! `HostEditor`.

use gpui::actions;

actions!(
    hosts,
    [
        /// Moves the cursor down a row (or to the next navigation entry).
        SelectNext,
        /// Moves the cursor up a row (or to the previous navigation entry).
        SelectPrevious,
        /// Moves the cursor to the first row.
        SelectFirst,
        /// Moves the cursor to the last row.
        SelectLast,
        /// Extends the selection down a row.
        ExtendNext,
        /// Extends the selection up a row.
        ExtendPrevious,
        /// Extends the selection to the first row.
        ExtendToFirst,
        /// Extends the selection to the last row.
        ExtendToLast,
        /// Adds the cursor's host to the selection or takes it out.
        ToggleSelected,
        /// Selects every host shown.
        SelectAll,
        /// Clears the selection.
        ClearSelection,
        /// Connects to the selected hosts, each in its own tab.
        ConnectSelected,
        /// Marks the selected hosts as favourites, or unmarks them.
        ToggleFavourite,
        /// Moves the selected hosts to the trash.
        TrashSelected,
        /// Chooses how the list is sorted.
        ChangeSort,
        /// Switches between list and grid view.
        ToggleLayout,
        /// Moves focus to the search field.
        FocusSearch,
        /// Moves focus to the host list.
        FocusList,
        /// Clears the search and returns to the list.
        ClearSearch,
        /// Shows what's inside the focused group.
        ExpandGroup,
        /// Hides what's inside the focused group.
        CollapseGroup,
        /// Puts the trash entry under the cursor back.
        Restore,
        /// Deletes the trash entry under the cursor for good.
        DeleteForever,
        /// Deletes everything in the trash for good.
        EmptyTrash,
        /// Opens the editor for a new host (in the group shown, if any).
        NewHost,
        /// Opens the editor for a new group (inside the group shown, if any).
        NewGroup,
        /// Edits the host under the cursor, the selected hosts together, or the group selected
        /// in the navigation column.
        EditSelected,
        /// Saves copies of the selected hosts.
        DuplicateSelected,
        /// Moves the selected hosts into a group.
        MoveToGroup,
        /// Moves the group selected in the navigation column, and everything in it, to the trash.
        TrashGroup,
        /// Saves the editor's changes.
        SaveEditor,
        /// Closes the editor without saving.
        CancelEditor,
    ]
);
