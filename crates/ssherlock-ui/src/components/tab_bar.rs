//! A row of tabs: [`TabBar`] lays them out, [`Tab`] draws one.
//!
//! Both are stateless. Whoever owns the tabs builds them each frame and attaches behaviour with
//! the usual gpui methods (`on_click`, `on_drag`, `on_drop`, …), which [`Tab`] forwards to its
//! root element.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    MouseButton, ParentElement, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::theme::ActiveTheme as _;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type CloseHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// One tab: a label and, optionally, a close button.
#[derive(IntoElement)]
pub struct Tab {
    root: Stateful<Div>,
    label: AnyElement,
    is_active: bool,
    on_close: Option<CloseHandler>,
}

impl Tab {
    /// A tab showing `label`.
    pub fn new(id: impl Into<ElementId>, label: impl IntoElement) -> Self {
        Self {
            root: div().id(id),
            label: label.into_any_element(),
            is_active: false,
            on_close: None,
        }
    }

    /// Draws the tab as the selected one.
    #[must_use]
    pub fn active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

    /// Shows the close button, which calls `handler`. Middle-clicking the tab does the same.
    #[must_use]
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl gpui::StatefulInteractiveElement for Tab {}

impl RenderOnce for Tab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let metrics = &theme.metrics;
        let hover = colors.tab_hover;
        let on_close = self.on_close;

        self.root
            .group("tab")
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.spacing.sm)
            .h_full()
            .min_w(metrics.tab_min_width)
            .max_w(metrics.tab_max_width)
            .pl(theme.spacing.lg)
            .pr(theme.spacing.sm)
            .border_r(metrics.border_width)
            .border_color(colors.border)
            .text_size(theme.text.small_size)
            .relative()
            .when(self.is_active, |d| {
                d.bg(colors.tab_active).text_color(colors.text).child(
                    // The accent bar along the top of the active tab.
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(metrics.tab_indicator)
                        .bg(colors.accent),
                )
            })
            .when(!self.is_active, |d| {
                d.bg(colors.tab_bar)
                    .text_color(colors.text_muted)
                    .hover(move |style| style.bg(hover))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.label),
            )
            .when_some(on_close.clone(), |d, on_close| {
                d.on_mouse_up(MouseButton::Middle, move |_, window, cx| {
                    on_close(window, cx);
                })
            })
            .when_some(on_close, |d, on_close| d.child(close_button(on_close, cx)))
    }
}

/// The small × at the end of a tab.
fn close_button(handler: CloseHandler, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let hover = theme.colors.control_hover;
    div()
        .id("close")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(theme.metrics.window_control_size)
        .rounded(theme.radius.sm)
        .text_color(theme.colors.text_muted)
        .hover(move |style| style.bg(hover))
        // Don't let the tab underneath start a drag or take the click.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            handler(window, cx);
        })
        .child("\u{d7}")
}

/// A horizontal strip of [`Tab`]s with an optional trailing button (usually "new tab").
#[derive(IntoElement)]
pub struct TabBar {
    root: Stateful<Div>,
    tabs: Vec<AnyElement>,
    end: Option<AnyElement>,
}

impl TabBar {
    /// An empty tab bar.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            root: div().id(id),
            tabs: Vec::new(),
            end: None,
        }
    }

    /// Adds tabs, left to right.
    #[must_use]
    pub fn tabs(mut self, tabs: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.tabs
            .extend(tabs.into_iter().map(IntoElement::into_any_element));
        self
    }

    /// A button after the last tab that calls `handler`, labelled `label`.
    #[must_use]
    pub fn end_button(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.end = Some(
            EndButton {
                label: label.into(),
                handler: Box::new(handler),
            }
            .into_any_element(),
        );
        self
    }
}

impl InteractiveElement for TabBar {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl gpui::StatefulInteractiveElement for TabBar {}

impl RenderOnce for TabBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        self.root
            .flex()
            .flex_none()
            .items_center()
            .h(theme.metrics.tab_bar_height)
            .bg(theme.colors.tab_bar)
            .border_b(theme.metrics.border_width)
            .border_color(theme.colors.border)
            .overflow_x_scroll()
            .children(self.tabs)
            .children(self.end)
    }
}

#[derive(IntoElement)]
struct EndButton {
    label: SharedString,
    handler: ClickHandler,
}

impl RenderOnce for EndButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let hover = theme.colors.control_hover;
        let handler = self.handler;
        div()
            .id("end-button")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(theme.metrics.window_control_size)
            .mx(theme.spacing.sm)
            .rounded(theme.radius.sm)
            .text_color(theme.colors.text_muted)
            .hover(move |style| style.bg(hover))
            .on_click(move |event, window, cx| handler(event, window, cx))
            .child(self.label)
    }
}
