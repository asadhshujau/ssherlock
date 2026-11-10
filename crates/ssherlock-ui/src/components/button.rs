//! A clickable text button.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Div, ElementId, InteractiveElement, Interactivity, IntoElement, ParentElement as _,
    RenderOnce, SharedString, Stateful, Styled as _, Window, div,
};

use crate::theme::ActiveTheme as _;

/// How prominent a button is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// The action the user most likely wants; filled with the accent colour.
    Primary,
    /// Everything else; only an outline.
    Subtle,
}

/// A button with a text label. Attach behaviour with `on_click`.
#[derive(IntoElement)]
pub struct Button {
    root: Stateful<Div>,
    label: SharedString,
    style: ButtonStyle,
}

impl Button {
    /// A subtle button labelled `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            root: div().id(id),
            label: label.into(),
            style: ButtonStyle::Subtle,
        }
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
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let hover = colors.control_hover;
        self.root
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
                ButtonStyle::Subtle => d
                    .border_color(colors.border)
                    .text_color(colors.text)
                    .hover(move |style| style.bg(hover)),
            })
            .child(self.label)
    }
}
