//! A single-line text field, for short inline edits such as renaming a tab.
//!
//! The editing rules live in [`EditBuffer`], plain data tested without gpui. [`TextInput`] is
//! the gpui entity around it. It takes typed characters from key events. It doesn't yet support
//! IME composition, mouse placement of the caret or a selection other than "everything", which
//! is all a rename field needs.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};

use crate::theme::ActiveTheme as _;

/// A change to an [`EditBuffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit<'a> {
    /// Type text at the caret, replacing the selection.
    Insert(&'a str),
    /// Delete the character before the caret, or the selection.
    Backspace,
    /// Delete the character after the caret, or the selection.
    Delete,
    /// Move the caret one character left.
    Left,
    /// Move the caret one character right.
    Right,
    /// Move the caret to the start.
    Home,
    /// Move the caret to the end.
    End,
}

/// Text being edited, with a caret and an optional select-all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditBuffer {
    text: String,
    /// Byte offset of the caret, always on a character boundary.
    cursor: usize,
    is_all_selected: bool,
}

impl EditBuffer {
    /// A buffer holding `text`, all selected, so typing replaces it.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            is_all_selected: true,
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text before and after the caret.
    pub fn split_at_cursor(&self) -> (&str, &str) {
        self.text.split_at(self.cursor)
    }

    /// Whether all the text is selected.
    pub fn is_all_selected(&self) -> bool {
        self.is_all_selected && !self.text.is_empty()
    }

    /// Applies `edit`.
    pub fn apply(&mut self, edit: Edit<'_>) {
        if self.is_all_selected() {
            self.is_all_selected = false;
            match edit {
                Edit::Insert(_) | Edit::Backspace | Edit::Delete => {
                    self.text.clear();
                    self.cursor = 0;
                    if let Edit::Insert(text) = edit {
                        self.insert(text);
                    }
                    return;
                }
                // Collapsing a selection moves the caret to that end of it.
                Edit::Left | Edit::Home => {
                    self.cursor = 0;
                    return;
                }
                Edit::Right | Edit::End => {
                    self.cursor = self.text.len();
                    return;
                }
            }
        }
        self.is_all_selected = false;
        match edit {
            Edit::Insert(text) => self.insert(text),
            Edit::Backspace => {
                if let Some(start) = self.previous_boundary() {
                    self.text.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
            }
            Edit::Delete => {
                if let Some(end) = self.next_boundary() {
                    self.text.replace_range(self.cursor..end, "");
                }
            }
            Edit::Left => self.cursor = self.previous_boundary().unwrap_or(0),
            Edit::Right => self.cursor = self.next_boundary().unwrap_or(self.text.len()),
            Edit::Home => self.cursor = 0,
            Edit::End => self.cursor = self.text.len(),
        }
    }

    /// Inserts `text` at the caret, dropping control characters: the field is one line.
    fn insert(&mut self, text: &str) {
        let clean: String = text.chars().filter(|ch| !ch.is_control()).collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.text
            .get(..self.cursor)?
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
    }

    fn next_boundary(&self) -> Option<usize> {
        let ch = self.text.get(self.cursor..)?.chars().next()?;
        Some(self.cursor + ch.len_utf8())
    }
}

/// What a [`TextInput`] reports when editing ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// Enter was pressed, or focus moved away: keep this text.
    Confirmed(String),
    /// Escape was pressed: discard the edit.
    Cancelled,
}

/// A focused one-line text field that reports once, when editing ends.
pub struct TextInput {
    buffer: EditBuffer,
    focus_handle: FocusHandle,
    /// Set once an event has been sent, so a blur after Enter or Escape sends nothing.
    is_finished: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    /// A field holding `text`, all selected and focused.
    pub fn new(text: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let blur = cx.on_blur(&focus_handle, window, |input, _, cx| input.confirm(cx));
        Self {
            buffer: EditBuffer::new(text),
            focus_handle,
            is_finished: false,
            _subscriptions: vec![blur],
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        self.finish(TextInputEvent::Confirmed(self.buffer.text().to_owned()), cx);
    }

    fn finish(&mut self, event: TextInputEvent, cx: &mut Context<Self>) {
        if !self.is_finished {
            self.is_finished = true;
            cx.emit(event);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let edit = match keystroke.key.as_str() {
            "enter" => {
                self.confirm(cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.finish(TextInputEvent::Cancelled, cx);
                cx.stop_propagation();
                return;
            }
            "backspace" => Edit::Backspace,
            "delete" => Edit::Delete,
            "left" => Edit::Left,
            "right" => Edit::Right,
            "home" => Edit::Home,
            "end" => Edit::End,
            _ => {
                let modifiers = keystroke.modifiers;
                let Some(text) = keystroke.key_char.as_deref() else {
                    return;
                };
                if modifiers.control || modifiers.platform {
                    return;
                }
                Edit::Insert(text)
            }
        };
        self.buffer.apply(edit);
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (before, after) = self.buffer.split_at_cursor();
        let is_focused = self.focus_handle.is_focused(window);
        let text = |s: &str| SharedString::from(s.to_owned());

        div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .items_center()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.colors.text)
            .when(self.buffer.is_all_selected(), |d| {
                d.child(
                    div()
                        .bg(theme.terminal.selection)
                        .child(text(self.buffer.text())),
                )
            })
            .when(!self.buffer.is_all_selected(), |d| {
                d.child(text(before))
                    .when(is_focused, |d| {
                        d.child(
                            div()
                                .w(theme.metrics.border_width)
                                .h(theme.text.ui_size)
                                .bg(theme.colors.accent),
                        )
                    })
                    .child(text(after))
            })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn state(buffer: &EditBuffer) -> (String, String, bool) {
        let (before, after) = buffer.split_at_cursor();
        (
            before.to_owned(),
            after.to_owned(),
            buffer.is_all_selected(),
        )
    }

    #[test]
    fn edits() {
        type Case<'a> = (&'a str, &'a str, &'a [Edit<'a>], (String, String, bool));
        let s = |before: &str, after: &str, all: bool| (before.to_owned(), after.to_owned(), all);
        let cases: &[Case] = &[
            ("starts all selected", "zsh", &[], s("zsh", "", true)),
            (
                "typing replaces all",
                "zsh",
                &[Edit::Insert("db")],
                s("db", "", false),
            ),
            (
                "backspace clears all",
                "zsh",
                &[Edit::Backspace],
                s("", "", false),
            ),
            (
                "left collapses to start",
                "zsh",
                &[Edit::Left],
                s("", "zsh", false),
            ),
            (
                "right collapses to end",
                "zsh",
                &[Edit::Right],
                s("zsh", "", false),
            ),
            (
                "insert in the middle",
                "ac",
                &[Edit::End, Edit::Left, Edit::Insert("b")],
                s("ab", "c", false),
            ),
            (
                "backspace and delete",
                "abcd",
                &[
                    Edit::Home,
                    Edit::Right,
                    Edit::Right,
                    Edit::Backspace,
                    Edit::Delete,
                ],
                s("a", "d", false),
            ),
            (
                "multi-byte characters",
                "日本",
                &[Edit::End, Edit::Left, Edit::Backspace, Edit::Insert("é")],
                s("é", "本", false),
            ),
            (
                "control characters dropped",
                "",
                &[Edit::Insert("a\nb\tc")],
                s("abc", "", false),
            ),
            (
                "edges are safe",
                "a",
                &[
                    Edit::Home,
                    Edit::Left,
                    Edit::Backspace,
                    Edit::End,
                    Edit::Right,
                    Edit::Delete,
                ],
                s("a", "", false),
            ),
        ];
        for (name, start, edits, expected) in cases {
            let mut buffer = EditBuffer::new(*start);
            for edit in *edits {
                buffer.apply(*edit);
            }
            assert_eq!(&state(&buffer), expected, "{name}");
        }
        assert!(!EditBuffer::new("").is_all_selected());
    }
}
