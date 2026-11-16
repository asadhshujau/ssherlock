//! A form panel over a view: a title, a scrolling body, and Cancel / Save. Editors for hosts,
//! keys and identities all open in one.

use gpui::{
    AnyElement, App, ClickEvent, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use super::button::{Button, ButtonStyle};
use crate::theme::Theme;

/// A panel titled `title` showing `body`, with Cancel and `save_label` buttons running
/// `on_cancel` and `on_save`. Its owner renders it over the view it edits and gives it a key
/// context binding Enter and Escape; the backdrop swallows clicks so nothing behind reacts.
pub fn sheet(
    theme: &Theme,
    title: impl Into<SharedString>,
    save_label: impl Into<SharedString>,
    body: AnyElement,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_save: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .p(theme.spacing.xl)
        .child(
            div()
                .id("sheet-backdrop")
                .absolute()
                .inset_0()
                .bg(theme.colors.modal_scrim)
                .occlude(),
        )
        .child(
            div()
                .id("sheet")
                // Without this the backdrop would count as hovered under the panel too.
                .occlude()
                .flex()
                .flex_col()
                .w(theme.metrics.editor_width)
                .max_h_full()
                .rounded(theme.radius.md)
                .bg(theme.colors.surface)
                .border(theme.metrics.border_width)
                .border_color(theme.colors.border)
                .text_size(theme.text.ui_size)
                .text_color(theme.colors.text)
                .child(
                    div()
                        .px(theme.spacing.xl)
                        .pt(theme.spacing.xl)
                        .pb(theme.spacing.lg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(
                    div()
                        .id("sheet-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(theme.spacing.xl)
                        .pb(theme.spacing.lg)
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.md)
                        .px(theme.spacing.xl)
                        .py(theme.spacing.lg)
                        .border_t(theme.metrics.border_width)
                        .border_color(theme.colors.border)
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme.text.small_size)
                                .text_color(theme.colors.text_muted)
                                .child(format!(
                                    "Enter or {}+Enter saves \u{00b7} Esc cancels \u{00b7} Tab moves between fields",
                                    crate::platform::SECONDARY_NAME
                                )),
                        )
                        .child(Button::new("sheet-cancel", "Cancel").on_click(on_cancel))
                        .child(
                            Button::new("sheet-save", save_label.into())
                                .style(ButtonStyle::Primary)
                                .on_click(on_save),
                        ),
                ),
        )
}
