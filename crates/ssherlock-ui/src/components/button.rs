//! A clickable text button.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, Div, ElementId, FocusHandle, InteractiveElement, Interactivity, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, div,
};

use super::form::Activate;
use crate::theme::ActiveTheme as _;

/// How prominent a button is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// The action the user most likely wants; filled with the accent colour.
    Primary,
    /// Everything else; only an outline.
    Subtle,
    /// A risky choice the user must mean, like trusting a changed host key.
    Danger,
}

/// A button with a text label. Attach behaviour with `on_click`.
///
/// A button given a focus handle ([`Button::focus_handle`]) can be reached with Tab and pressed
/// with Enter or Space. gpui clicks a focused element when either key is released, so
/// `on_click` covers the keyboard too. The keymap binds the key-down to
/// [`form::Activate`](super::form::Activate), which the button swallows so Enter doesn't also
/// reach a binding further out (like an editor's "Enter saves").
///
/// A keyboard click counts only if the key went down on the button too. Otherwise Enter that
/// picks an item in a picker would, on release, click the dropdown button focus returns to and
/// open the picker again.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    root: Stateful<Div>,
    on_click: Option<ClickHandler>,
    label: SharedString,
    style: ButtonStyle,
    focus_handle: Option<FocusHandle>,
    /// Drawn like a select: label on the left, a down arrow on the right.
    is_dropdown: bool,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

impl Button {
    /// A subtle button labelled `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            root: div().id(id.clone()),
            id,
            on_click: None,
            label: label.into(),
            style: ButtonStyle::Subtle,
            focus_handle: None,
            is_dropdown: false,
        }
    }

    /// Draws the button like a select field: its current value on the left and a down arrow
    /// on the right. For buttons that open a picker.
    #[must_use]
    pub fn dropdown(mut self) -> Self {
        self.is_dropdown = true;
        self
    }

    /// Makes the button focusable through `handle` (make it a tab stop so Tab reaches it).
    #[must_use]
    pub fn focus_handle(mut self, handle: &FocusHandle) -> Self {
        self.focus_handle = Some(handle.clone());
        self
    }

    /// Runs `handler` when the button is clicked, or pressed with Enter or Space while focused.
    #[must_use]
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Sets how prominent the button is.
    #[must_use]
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl gpui::StatefulInteractiveElement for Button {}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hover = cx.theme().colors.control_hover;
        let is_focused = self
            .focus_handle
            .as_ref()
            .is_some_and(|handle| handle.is_focused(window));
        // Whether Enter or Space went down on this button, so its release may click it.
        let pressed = self
            .focus_handle
            .is_some()
            .then(|| window.use_keyed_state(self.id.clone(), cx, |_, _| false));
        let on_click = self.on_click.map(|handler| {
            let pressed = pressed.clone();
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                if let (ClickEvent::Keyboard(_), Some(pressed)) = (event, &pressed) {
                    let was_pressed = pressed.update(cx, |p, _| std::mem::take(p));
                    if !was_pressed {
                        return;
                    }
                }
                handler(event, window, cx);
            }
        });
        let theme = cx.theme();
        let colors = &theme.colors;
        self.root
            .when_some(self.focus_handle, |d, handle| {
                d.key_context("Button").track_focus(&handle).on_action(
                    move |_: &Activate, _, cx| {
                        if let Some(pressed) = &pressed {
                            pressed.update(cx, |p, _| *p = true);
                        }
                    },
                )
            })
            .when_some(on_click, |d, on_click| d.on_click(on_click))
            .flex()
            .items_center()
            .justify_center()
            .px(theme.spacing.lg)
            .py(theme.spacing.sm)
            .rounded(theme.radius.sm)
            .border(theme.metrics.border_width)
            .text_size(theme.text.ui_size)
            .map(|d| match self.style {
                ButtonStyle::Primary => d
                    .bg(colors.accent)
                    .border_color(colors.accent)
                    .text_color(colors.accent_text),
                ButtonStyle::Danger => d
                    .bg(colors.danger)
                    .border_color(colors.danger)
                    .text_color(colors.danger_text),
                ButtonStyle::Subtle => d
                    .border_color(colors.border)
                    .text_color(colors.text)
                    .hover(move |style| style.bg(hover)),
            })
            .when(is_focused, |d| d.border_color(colors.accent))
            .when(self.is_dropdown, |d| {
                d.justify_between()
                    .px(theme.spacing.md)
                    .child(self.label.clone())
                    .child(div().text_color(colors.text_muted).child("\u{25be}"))
            })
            .when(!self.is_dropdown, |d| d.child(self.label))
    }
}
