//! Form layout: labelled fields in sections, with Tab moving between them.
//!
//! Every editor (hosts, groups, and later keys, snippets, rules) is built from these, so they
//! all look and behave the same (CLAUDE.md §7). The controls inside are ordinary components
//! ([`TextInput::field`](super::TextInput::field), [`Checkbox`](super::Checkbox),
//! [`SegmentedControl`](super::SegmentedControl), …); their focus handles are tab stops, so Tab
//! and Shift+Tab walk through them in the order they're drawn.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled as _, Window, actions, div,
};

use crate::theme::ActiveTheme as _;

actions!(
    form,
    [
        /// Moves focus to the next field.
        FocusNext,
        /// Moves focus to the previous field.
        FocusPrevious,
        /// Turns the focused checkbox on or off.
        Toggle,
        /// Presses the focused button.
        Activate,
    ]
);

/// A form: sections of fields, top to bottom. Its key context `Form` binds Tab and Shift+Tab.
#[derive(IntoElement, Default)]
pub struct Form {
    children: Vec<AnyElement>,
}

impl Form {
    /// An empty form; add [`FormSection`]s or [`FormField`]s as children.
    pub fn new() -> Self {
        Self::default()
    }
}

impl ParentElement for Form {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Form {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .key_context("Form")
            .on_action(|_: &FocusNext, window, _| window.focus_next())
            .on_action(|_: &FocusPrevious, window, _| window.focus_prev())
            .flex()
            .flex_col()
            .gap(theme.spacing.xl)
            .text_size(theme.text.ui_size)
            .text_color(theme.colors.text)
            .children(self.children)
    }
}

/// A titled group of fields.
#[derive(IntoElement)]
pub struct FormSection {
    title: SharedString,
    children: Vec<AnyElement>,
}

impl FormSection {
    /// A section headed `title`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            children: Vec::new(),
        }
    }
}

impl ParentElement for FormSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for FormSection {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.md)
            .child(
                div()
                    .text_size(theme.text.small_size)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.colors.text_muted)
                    .child(self.title.to_uppercase()),
            )
            .children(self.children)
    }
}

/// One labelled control, with an optional hint below it, replaced by an error when there is
/// one.
#[derive(IntoElement)]
pub struct FormField {
    label: SharedString,
    control: AnyElement,
    hint: Option<SharedString>,
    error: Option<SharedString>,
}

impl FormField {
    /// `control` labelled `label`.
    pub fn new(label: impl Into<SharedString>, control: impl IntoElement) -> Self {
        Self {
            label: label.into(),
            control: control.into_any_element(),
            hint: None,
            error: None,
        }
    }

    /// Explains the field below it, e.g. where an inherited value comes from.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Shows what's wrong with the value, in place of the hint.
    #[must_use]
    pub fn error(mut self, error: Option<impl Into<SharedString>>) -> Self {
        self.error = error.map(Into::into);
        self
    }
}

impl RenderOnce for FormField {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let note = self.error.clone().or(self.hint);
        div()
            .flex()
            .gap(theme.spacing.lg)
            .child(
                div()
                    .flex_none()
                    .w(theme.metrics.form_label_width)
                    .pt(theme.spacing.sm)
                    .text_color(theme.colors.text_muted)
                    .child(self.label),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.spacing.xs)
                    .child(self.control)
                    .when_some(note, |d, note| {
                        d.child(
                            div()
                                .text_size(theme.text.small_size)
                                .text_color(if self.error.is_some() {
                                    theme.colors.danger
                                } else {
                                    theme.colors.text_muted
                                })
                                .child(note),
                        )
                    }),
            )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use gpui::{
        AppContext as _, Context, Entity, FocusHandle, Focusable as _, Render, TestAppContext,
    };
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::components::{Checkbox, Segment, SegmentedControl, TextInput};

    /// A form with two text fields, a checkbox and a segmented control.
    struct Sample {
        first: Entity<TextInput>,
        second: Entity<TextInput>,
        checkbox: FocusHandle,
        is_checked: bool,
        segments: FocusHandle,
        selected: usize,
    }

    impl Render for Sample {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                Form::new().child(
                    FormSection::new("Basics")
                        .child(FormField::new("First", self.first.clone()))
                        .child(FormField::new("Second", self.second.clone()).hint("a hint"))
                        .child(FormField::new(
                            "Check",
                            Checkbox::new("check", "On", self.is_checked, &self.checkbox)
                                .on_toggle(cx.listener(|this, value, _, cx| {
                                    this.is_checked = *value;
                                    cx.notify();
                                })),
                        ))
                        .child(FormField::new(
                            "Pick",
                            SegmentedControl::new(
                                "pick",
                                vec![Segment::new("A"), Segment::new("B"), Segment::new("C")],
                                self.selected,
                                &self.segments,
                            )
                            .on_select(cx.listener(
                                |this, index, _, cx| {
                                    this.selected = *index;
                                    cx.notify();
                                },
                            )),
                        )),
                ),
            )
        }
    }

    /// Presses and releases `key`, as a keyboard does. `simulate_keystrokes` sends no release,
    /// and gpui clicks a focused element on the release of Enter or Space.
    pub(crate) fn press(cx: &mut gpui::VisualTestContext, key: &str) {
        cx.simulate_keystrokes(key);
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
        });
        cx.run_until_parked();
    }

    /// Regression: Space toggled a checkbox on the key-down and the key-up's click toggled it
    /// back, so on a real keyboard it did nothing. Each full press must toggle once.
    #[gpui::test]
    fn each_press_of_space_toggles_a_checkbox_once(cx: &mut TestAppContext) {
        let (view, cx) = sample(cx);
        cx.update(|window, cx| view.read(cx).checkbox.focus(window));
        press(cx, "space");
        assert!(view.read_with(cx, |view, _| view.is_checked));
        press(cx, "space");
        assert!(!view.read_with(cx, |view, _| view.is_checked));
    }

    fn sample(cx: &mut TestAppContext) -> (Entity<Sample>, &mut gpui::VisualTestContext) {
        cx.update(crate::keymap::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let first = cx.new(|cx| TextInput::field("", cx));
            first.focus_handle(cx).focus(window);
            Sample {
                first,
                second: cx.new(|cx| TextInput::field("", cx)),
                checkbox: cx.focus_handle().tab_stop(true),
                is_checked: false,
                segments: cx.focus_handle().tab_stop(true),
                selected: 0,
            }
        });
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn tab_walks_the_fields_and_each_control_takes_its_keys(cx: &mut TestAppContext) {
        let (view, cx) = sample(cx);
        cx.simulate_input("a");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("b");
        cx.simulate_keystrokes("tab");
        press(cx, "space");
        cx.simulate_keystrokes("tab right right");
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(view.first.read(cx).text(), "a");
            assert_eq!(view.second.read(cx).text(), "b");
            assert!(view.is_checked);
            assert_eq!(view.selected, 2);
        });
        // Wraps around from the last control, and Shift+Tab goes back.
        cx.simulate_keystrokes("right tab shift-tab left");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.selected, 2));
    }
}
