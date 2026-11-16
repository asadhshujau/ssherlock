//! A host drawn as a list row or a grid card. Stateless: the list builds these each frame and
//! attaches the mouse handlers.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement, Interactivity, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Stateful, Styled as _, Window, div,
};
use ssherlock_core::hosts::{Colour, Host};

use crate::theme::{ActiveTheme as _, Theme};

/// How the list shows hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// One host per row.
    #[default]
    List,
    /// Cards that wrap.
    Grid,
}

/// What a row shows about a host. Built by the list, which knows the inherited values.
#[derive(Clone, Debug)]
pub struct HostSummary {
    /// The title.
    pub title: SharedString,
    /// `user@address:port`, with inherited parts filled in.
    pub target: SharedString,
    /// The effective colour mark.
    pub colour: Option<Colour>,
    /// Tags.
    pub tags: Vec<SharedString>,
    /// Starred.
    pub is_favourite: bool,
}

impl HostSummary {
    /// The summary of `host`, with `target` and `colour` already resolved.
    pub fn new(host: &Host, target: String, colour: Option<Colour>) -> Self {
        Self {
            title: host.title().into_owned().into(),
            target: target.into(),
            colour,
            tags: host
                .tags
                .iter()
                .map(|t| SharedString::from(format!("#{t}")))
                .collect(),
            is_favourite: host.is_favourite,
        }
    }
}

/// One host as a row or a card.
#[derive(IntoElement)]
pub struct HostItem {
    root: Stateful<Div>,
    summary: HostSummary,
    layout: Layout,
    is_selected: bool,
    is_cursor: bool,
    /// Whether the list has focus; the cursor is drawn strongly only then.
    is_focused: bool,
}

impl HostItem {
    /// `summary` drawn in `layout`.
    pub fn new(id: impl Into<ElementId>, summary: HostSummary, layout: Layout) -> Self {
        Self {
            root: div().id(id),
            summary,
            layout,
            is_selected: false,
            is_cursor: false,
            is_focused: false,
        }
    }

    /// Draws it as selected.
    #[must_use]
    pub fn selected(mut self, is_selected: bool) -> Self {
        self.is_selected = is_selected;
        self
    }

    /// Draws the keyboard cursor on it; strongly if the list is focused.
    #[must_use]
    pub fn cursor(mut self, is_cursor: bool, is_focused: bool) -> Self {
        self.is_cursor = is_cursor;
        self.is_focused = is_focused;
        self
    }
}

impl InteractiveElement for HostItem {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl gpui::StatefulInteractiveElement for HostItem {}

impl RenderOnce for HostItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = &theme.colors;
        let hover = colors.control_hover;
        let summary = self.summary;
        let title = div()
            .flex()
            .items_center()
            .gap(theme.spacing.sm)
            .min_w_0()
            .child(mark(&theme, summary.colour))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .child(summary.title.clone()),
            )
            .when(summary.is_favourite, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(colors.accent)
                        .child("\u{2605}"),
                )
            });
        // Ellipsis text needs a definite width to cut at; inside a column it only gets one as
        // the flexible child of a row, as the title has.
        let target = div().flex().w_full().min_w_0().child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(theme.text.small_size)
                .text_color(colors.text_muted)
                .child(summary.target.clone()),
        );
        let tags = tag_chips(&theme, &summary.tags);

        let base = self
            .root
            .cursor_pointer()
            .rounded(theme.radius.sm)
            .border(theme.metrics.border_width)
            .text_size(theme.text.ui_size)
            .text_color(colors.text)
            .border_color(if self.is_cursor && self.is_focused {
                colors.accent
            } else if self.is_cursor {
                colors.border
            } else {
                gpui::transparent_black()
            })
            .when(self.is_selected, |d| d.bg(colors.row_selected))
            .when(!self.is_selected, |d| d.hover(move |s| s.bg(hover)));
        match self.layout {
            Layout::List => base
                .flex()
                .items_center()
                .gap(theme.spacing.lg)
                .px(theme.spacing.md)
                .py(theme.spacing.sm)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(title)
                        .child(target),
                )
                .child(tags),
            Layout::Grid => base
                .flex()
                .flex_col()
                .gap(theme.spacing.sm)
                .w(theme.metrics.card_width)
                .p(theme.spacing.lg)
                .when(!self.is_selected, |d| d.bg(colors.surface))
                .child(title)
                .child(target)
                .child(tags),
        }
    }
}

/// A host's colour mark: a small dot, or the same space left empty so titles line up.
pub fn mark(theme: &Theme, colour: Option<Colour>) -> AnyElement {
    div()
        .flex_none()
        .size(theme.metrics.status_dot)
        .rounded_full()
        .when_some(colour, |d, colour| d.bg(theme.colors.marks.get(colour)))
        .into_any_element()
}

/// Tags as small rounded chips.
pub fn tag_chips(theme: &Theme, tags: &[SharedString]) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(theme.spacing.xs)
        .children(tags.iter().map(|tag| {
            div()
                .px(theme.spacing.sm)
                .rounded(theme.radius.sm)
                .bg(theme.colors.control_hover)
                .text_size(theme.text.small_size)
                .text_color(theme.colors.text_muted)
                .child(tag.clone())
        }))
}
