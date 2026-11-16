//! A text field: short inline edits (renaming a tab), filters that react as you type (search),
//! and form fields, single-line or multi-line.
//!
//! The editing rules live in [`EditBuffer`], plain data tested without gpui. [`TextInput`] is
//! the gpui entity around it. It takes typed characters from key events and handles paste, copy,
//! cut and select-all as actions bound in the keymap. It doesn't yet support IME composition,
//! mouse placement of the caret or a selection other than "everything".
//!
//! A [`TextInput::secret`] field (passwords, passphrases) shows a bullet per character, never puts
//! its text in an event, hands it over once as a [`SecretString`], and zeroes its buffer on drop.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyContext, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, actions, div,
};
use ssherlock_core::SecretString;
use zeroize::Zeroize as _;

use crate::theme::ActiveTheme as _;

actions!(
    text_input,
    [
        /// Inserts the clipboard's text at the caret.
        Paste,
        /// Copies the selected text.
        Copy,
        /// Copies the selected text and deletes it.
        Cut,
        /// Selects all the text.
        SelectAll,
        /// Starts a new line in a multi-line field.
        Newline,
    ]
);

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
    /// Start a new line (multi-line buffers only).
    Newline,
    /// Move the caret to the line above, keeping its column where possible.
    Up,
    /// Move the caret to the line below, keeping its column where possible.
    Down,
}

/// Text being edited, with a caret and an optional select-all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditBuffer {
    text: String,
    /// Byte offset of the caret, always on a character boundary.
    cursor: usize,
    is_all_selected: bool,
    /// Whether the text may hold line breaks.
    is_multiline: bool,
}

impl EditBuffer {
    /// A buffer holding `text`, all selected, so typing replaces it.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            is_all_selected: true,
            is_multiline: false,
        }
    }

    /// A buffer holding `text` with the caret at its end and nothing selected, as a form field
    /// starts.
    pub fn with_caret_at_end(text: impl Into<String>) -> Self {
        Self {
            is_all_selected: false,
            ..Self::new(text)
        }
    }

    /// Lets the text hold line breaks: [`Edit::Newline`] works and pasted line breaks are kept.
    #[must_use]
    pub fn multiline(mut self) -> Self {
        self.is_multiline = true;
        self
    }

    /// Replaces the text, leaving the caret at its end and nothing selected.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text.zeroize();
        self.text = text.into();
        self.cursor = self.text.len();
        self.is_all_selected = false;
    }

    /// Selects all the text.
    pub fn select_all(&mut self) {
        self.is_all_selected = true;
        self.cursor = self.text.len();
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
                Edit::Insert(_) | Edit::Backspace | Edit::Delete | Edit::Newline => {
                    self.text.clear();
                    self.cursor = 0;
                    match edit {
                        Edit::Insert(text) => self.insert(text),
                        Edit::Newline => self.newline(),
                        _ => {}
                    }
                    return;
                }
                // Collapsing a selection moves the caret to that end of it.
                Edit::Left | Edit::Home | Edit::Up => {
                    self.cursor = 0;
                    return;
                }
                Edit::Right | Edit::End | Edit::Down => {
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
            Edit::Newline => self.newline(),
            Edit::Up => self.move_line(false),
            Edit::Down => self.move_line(true),
        }
    }

    /// The text split into lines, the caret's line index, and the caret's byte offset within it.
    pub fn lines_with_cursor(&self) -> (Vec<&str>, usize, usize) {
        let before = self.text.get(..self.cursor).unwrap_or(&self.text);
        let line = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        (
            self.text.split('\n').collect(),
            line,
            self.cursor - line_start,
        )
    }

    /// Inserts `text` at the caret, dropping control characters other than (in a multi-line
    /// buffer) line breaks.
    fn insert(&mut self, text: &str) {
        let multiline = self.is_multiline;
        let clean: String = text
            .replace("\r\n", "\n")
            .chars()
            .filter(|&ch| !ch.is_control() || (multiline && ch == '\n'))
            .collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn newline(&mut self) {
        if self.is_multiline {
            self.text.insert(self.cursor, '\n');
            self.cursor += 1;
        }
    }

    /// Moves the caret to the same character column on the next or previous line, or to the
    /// start or end of the text when there's no such line.
    fn move_line(&mut self, down: bool) {
        let (lines, line, offset) = self.lines_with_cursor();
        let column = lines
            .get(line)
            .and_then(|text| text.get(..offset))
            .map_or(0, |text| text.chars().count());
        let target = if down {
            line + 1
        } else if let Some(previous) = line.checked_sub(1) {
            previous
        } else {
            self.cursor = 0;
            return;
        };
        let Some(target_text) = lines.get(target) else {
            self.cursor = self.text.len();
            return;
        };
        let start: usize = lines.iter().take(target).map(|l| l.len() + 1).sum();
        let within = target_text
            .char_indices()
            .nth(column)
            .map_or(target_text.len(), |(at, _)| at);
        self.cursor = start + within;
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
    /// Drawn with a border that lights up when focused, as form fields are.
    is_framed: bool,
    /// Shown, faded, while the field is empty.
    placeholder: Option<SharedString>,
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
        let focus_handle = cx.focus_handle().tab_stop(true);
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
            is_framed: false,
            placeholder: None,
            focus_handle,
            is_finished: false,
            _subscriptions: subscriptions,
        }
    }

    /// A framed form field holding `text`, caret at the end, not focused: a form has many and
    /// focuses one itself. Tab moves between them.
    pub fn field(text: impl Into<String>, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: EditBuffer::with_caret_at_end(text),
            kind: InputKind::Persistent,
            is_secret: false,
            is_framed: true,
            placeholder: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            is_finished: false,
            _subscriptions: Vec::new(),
        }
    }

    /// Shows `text`, faded, while the field is empty: an example, or the inherited value.
    #[must_use]
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Changes the placeholder.
    pub fn set_placeholder(&mut self, text: Option<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text;
        cx.notify();
    }

    /// Lets the field hold several lines: Enter starts a new line.
    #[must_use]
    pub fn multiline(mut self) -> Self {
        self.buffer.is_multiline = true;
        self
    }

    /// Replaces the text, e.g. when a form is reset. Emits [`TextInputEvent::Changed`].
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.buffer.set_text(text);
        cx.emit(TextInputEvent::Changed(self.event_text()));
        cx.notify();
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
        self.buffer.select_all();
        cx.notify();
    }

    /// Applies `edit`, then reports and redraws if the text changed.
    fn edit(&mut self, edit: Edit<'_>, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(Edit::Insert(&text), cx);
        }
    }

    /// The selected text, if it may leave the field: never for a secret.
    fn selection(&self) -> Option<String> {
        (self.buffer.is_all_selected() && !self.is_secret).then(|| self.buffer.text().to_owned())
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.edit(Edit::Delete, cx);
        }
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(Edit::Newline, cx);
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
        let is_multiline = self.buffer.is_multiline;
        let edit = match keystroke.key.as_str() {
            // Tab moves between fields; whoever owns the form handles it.
            "tab" => return,
            "enter" if is_multiline => Edit::Newline,
            "up" if is_multiline => Edit::Up,
            "down" if is_multiline => Edit::Down,
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
        self.edit(edit, cx);
        cx.stop_propagation();
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
        let theme = cx.theme().clone();
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
        let caret = || {
            div()
                .flex_none()
                .w(theme.metrics.border_width)
                .h(theme.text.ui_size)
                .bg(theme.colors.accent)
        };
        let mut context = KeyContext::new_with_defaults();
        context.add("TextInput");
        if self.buffer.is_multiline {
            context.add("multiline");
        }

        let body = if self.buffer.text().is_empty() {
            div()
                .flex()
                .items_center()
                .when(is_focused, |d| d.child(caret()))
                .when_some(self.placeholder.clone(), |d, placeholder| {
                    d.child(div().text_color(theme.colors.text_muted).child(placeholder))
                })
        } else if self.buffer.is_all_selected() {
            div().child(
                div()
                    .bg(theme.terminal.selection)
                    .child(text(self.buffer.text())),
            )
        } else if self.buffer.is_multiline {
            let (lines, caret_line, offset) = self.buffer.lines_with_cursor();
            div()
                .flex()
                .flex_col()
                .children(lines.into_iter().enumerate().map(|(index, line)| {
                    let row = div().flex().items_center().min_h(theme.text.ui_size);
                    if index == caret_line && is_focused {
                        let (before, after) = line.split_at(offset.min(line.len()));
                        row.child(text(before)).child(caret()).child(text(after))
                    } else {
                        row.child(text(line))
                    }
                }))
        } else {
            let (before, after) = self.buffer.split_at_cursor();
            div()
                .flex()
                .items_center()
                .child(text(before))
                .when(is_focused, |d| d.child(caret()))
                .child(text(after))
        };

        div()
            .key_context(context)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::select_all_action))
            .on_action(cx.listener(Self::newline))
            .flex()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.colors.text)
            .when(self.is_framed, |d| {
                d.px(theme.spacing.md)
                    .py(theme.spacing.sm)
                    .rounded(theme.radius.sm)
                    .bg(theme.colors.background)
                    .border(theme.metrics.border_width)
                    .border_color(if is_focused {
                        theme.colors.accent
                    } else {
                        theme.colors.border
                    })
            })
            .when(self.buffer.is_multiline, |d| {
                d.items_start().min_h(theme.metrics.multiline_min_height)
            })
            .when(!self.buffer.is_multiline, gpui::Styled::items_center)
            .child(body)
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

    #[test]
    fn form_field_buffers_and_multiline_edits() {
        let s = |before: &str, after: &str, all: bool| (before.to_owned(), after.to_owned(), all);
        assert_eq!(
            state(&EditBuffer::with_caret_at_end("abc")),
            s("abc", "", false)
        );

        let mut single = EditBuffer::with_caret_at_end("");
        single.apply(Edit::Insert("one\ntwo"));
        single.apply(Edit::Newline);
        assert_eq!(single.text(), "onetwo", "one line drops line breaks");

        let mut multi = EditBuffer::with_caret_at_end("").multiline();
        multi.apply(Edit::Insert("first\r\nsecond"));
        multi.apply(Edit::Newline);
        multi.apply(Edit::Insert("x"));
        assert_eq!(multi.text(), "first\nsecond\nx");

        // Up and Down keep the column, clamped to the line's length.
        let mut lines = EditBuffer::with_caret_at_end("long line\nab\nanother").multiline();
        lines.apply(Edit::Up);
        assert_eq!(
            lines.split_at_cursor().1,
            "\nanother",
            "end of the short line"
        );
        lines.apply(Edit::Up);
        assert_eq!(lines.split_at_cursor().0, "lo");
        lines.apply(Edit::Up);
        assert_eq!(
            lines.split_at_cursor().0,
            "",
            "up from the first line goes to the start"
        );
        lines.apply(Edit::Right);
        lines.apply(Edit::Down);
        lines.apply(Edit::Down);
        assert_eq!(lines.split_at_cursor().1, "nother");
        lines.apply(Edit::Down);
        assert_eq!(
            lines.split_at_cursor().1,
            "",
            "down from the last line goes to the end"
        );
        assert_eq!(
            lines.lines_with_cursor(),
            (vec!["long line", "ab", "another"], 2, 7)
        );

        let mut replaced = EditBuffer::new("old");
        replaced.set_text("new");
        assert_eq!(state(&replaced), s("new", "", false));
        replaced.select_all();
        assert!(replaced.is_all_selected());
    }

    #[gpui::test]
    fn paste_copy_and_cut(cx: &mut gpui::TestAppContext) {
        cx.update(crate::keymap::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            let field = TextInput::field("host", cx);
            window.focus(&field.focus_handle);
            field
        });
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string(".example.com\n".into()));
        cx.simulate_keystrokes("secondary-v");
        cx.run_until_parked();
        assert_eq!(
            input.read_with(cx, |input, _| input.text().to_owned()),
            "host.example.com"
        );
        cx.simulate_keystrokes("secondary-a secondary-x");
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |input, _| input.text().to_owned()), "");
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("host.example.com")
        );
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
