//! A checkbox with a label.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use super::form::Toggle;
use crate::theme::ActiveTheme as _;

type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// A checkbox. Stateless: its owner keeps the value, passes it in each frame, and gets the new
/// value from `on_toggle` when it's clicked or Space is pressed while it's focused.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: SharedString,
    is_checked: bool,
    focus_handle: FocusHandle,
    on_toggle: Option<ToggleHandler>,
}

impl Checkbox {
    /// A checkbox labelled `label`, focusable through `focus_handle` (make it a tab stop so Tab
    /// reaches it).
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        is_checked: bool,
        focus_handle: &FocusHandle,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            is_checked,
            focus_handle: focus_handle.clone(),
            on_toggle: None,
        }
    }

    /// Called with the new value.
    #[must_use]
    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let is_focused = self.focus_handle.is_focused(window);
        let next = !self.is_checked;
        let on_click = self.on_toggle.clone();
        let on_key = self.on_toggle;
        div()
            .id(self.id)
            .key_context("Checkbox")
            .track_focus(&self.focus_handle)
            .when_some(on_key, |d, handler| {
                d.on_action(move |_: &Toggle, window, cx| handler(&next, window, cx))
            })
            .when_some(on_click, |d, handler| {
                d.on_click(move |_, window, cx| handler(&next, window, cx))
            })
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(theme.text.ui_size)
                    .rounded(theme.radius.sm)
                    .border(theme.metrics.border_width)
                    .border_color(if is_focused {
                        theme.colors.accent
                    } else {
                        theme.colors.border
                    })
                    .when(self.is_checked, |d| {
                        d.bg(theme.colors.accent)
                            .text_color(theme.colors.accent_text)
                            .text_size(theme.text.small_size)
                            .child("\u{2713}")
                    }),
            )
            .child(self.label)
    }
}
