//! A single-line text field, for short inline edits (renaming a tab) and filters that react
//! as you type (search).
//!
//! The editing rules live in [`EditBuffer`], plain data tested without gpui. [`TextInput`] is
//! the gpui entity around it. It takes typed characters from key events. It doesn't yet support
//! IME composition, mouse placement of the caret or a selection other than "everything", which
//! is all a rename field needs.
//!
//! A [`TextInput::secret`] field (passwords, passphrases) shows a bullet per character, never puts
//! its text in an event, hands it over once as a [`SecretString`], and zeroes its buffer on drop.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};
use ssherlock_core::SecretString;
use zeroize::Zeroize as _;

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

    /// Wipes the text from memory and empties the buffer.
    pub fn zeroize(&mut self) {
        self.text.zeroize();
        self.cursor = 0;
        self.is_all_selected = false;
    }

    /// Takes the text out, leaving the buffer empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.is_all_selected = false;
        std::mem::take(&mut self.text)
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

/// What a [`TextInput`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The text changed; carries the new text (empty for a secret field).
    Changed(String),
    /// Enter was pressed (or, for a [`InputKind::SingleEdit`] field, focus moved away): keep
    /// this text (empty for a secret field; take it with [`TextInput::take_secret`]).
    Confirmed(String),
    /// Escape was pressed: discard the edit.
    Cancelled,
}

/// How long a [`TextInput`] lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// Edited once, like a rename: it reports `Confirmed` or `Cancelled` once, and clicking
    /// away confirms.
    SingleEdit,
    /// Stays open, like a search box: Enter and Escape may be pressed many times (or bound to
    /// actions instead), and losing focus does nothing.
    Persistent,
}

/// A focused one-line text field.
pub struct TextInput {
    buffer: EditBuffer,
    kind: InputKind,
    is_secret: bool,
    focus_handle: FocusHandle,
    /// Set once a [`InputKind::SingleEdit`] field has reported, so a later blur sends nothing.
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
    pub fn new(
        text: impl Into<String>,
        kind: InputKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let mut subscriptions = Vec::new();
        if kind == InputKind::SingleEdit {
            subscriptions.push(cx.on_blur(&focus_handle, window, |input, _, cx| {
                input.confirm(cx);
            }));
        }
        Self {
            buffer: EditBuffer::new(text),
            kind,
            is_secret: false,
            focus_handle,
            is_finished: false,
            _subscriptions: subscriptions,
        }
    }

    /// Makes this a field for a secret: masked on screen and kept out of events.
    #[must_use]
    pub fn secret(mut self) -> Self {
        self.is_secret = true;
        self
    }

    /// The current text (empty for a secret field).
    pub fn text(&self) -> &str {
        if self.is_secret {
            ""
        } else {
            self.buffer.text()
        }
    }

    /// Hands over a secret field's text and empties the field.
    pub fn take_secret(&mut self, cx: &mut Context<Self>) -> SecretString {
        let secret = SecretString::from(self.buffer.take());
        cx.notify();
        secret
    }

    /// The text an event may carry: the text itself, or nothing for a secret field.
    fn event_text(&self) -> String {
        if self.is_secret {
            String::new()
        } else {
            self.buffer.text().to_owned()
        }
    }

    /// Selects all the text, so typing replaces it.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.buffer = EditBuffer::new(self.buffer.text());
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        self.finish(TextInputEvent::Confirmed(self.event_text()), cx);
    }

    fn finish(&mut self, event: TextInputEvent, cx: &mut Context<Self>) {
        match self.kind {
            InputKind::Persistent => cx.emit(event),
            InputKind::SingleEdit if !self.is_finished => {
                self.is_finished = true;
                cx.emit(event);
            }
            InputKind::SingleEdit => {}
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
        let previous_len = self.buffer.text().len();
        let previous = (!self.is_secret).then(|| self.buffer.text().to_owned());
        self.buffer.apply(edit);
        let changed = match &previous {
            Some(previous) => previous != self.buffer.text(),
            None => previous_len != self.buffer.text().len() || matches!(edit, Edit::Insert(_)),
        };
        if changed {
            cx.emit(TextInputEvent::Changed(self.event_text()));
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        if self.is_secret {
            self.buffer.zeroize();
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (before, after) = self.buffer.split_at_cursor();
        let is_focused = self.focus_handle.is_focused(window);
        let is_secret = self.is_secret;
        // A secret shows one bullet per character, so its length is all that's visible.
        let text = |s: &str| {
            if is_secret {
                SharedString::from("\u{2022}".repeat(s.chars().count()))
            } else {
                SharedString::from(s.to_owned())
            }
        };

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

    /// A secret field's text never leaves it except through `take_secret`.
    #[gpui::test]
    fn secret_text_stays_out_of_events(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        use ssherlock_core::ExposeSecret as _;

        let (input, cx) = cx.add_window_view(|window, cx| {
            TextInput::new("", InputKind::Persistent, window, cx).secret()
        });
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&input, move |_, event: &TextInputEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        cx.simulate_input("hunter2");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        let events = seen.take();
        for event in &events {
            assert!(
                !format!("{event:?}").contains("hunter"),
                "leaked: {event:?}"
            );
        }
        assert!(matches!(events.last(), Some(TextInputEvent::Confirmed(text)) if text.is_empty()));
        assert_eq!(input.read_with(cx, |input, _| input.text().to_owned()), "");
        let secret = input.update(cx, TextInput::take_secret);
        assert_eq!(secret.expose_secret(), "hunter2");
        let again = input.update(cx, TextInput::take_secret);
        assert_eq!(again.expose_secret(), "", "taking empties the field");
    }
}
