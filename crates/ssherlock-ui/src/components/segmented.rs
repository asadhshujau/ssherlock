//! A row of mutually exclusive choices, like a radio group drawn as joined buttons.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, FocusHandle, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use super::context_menu::{SelectNext, SelectPrevious};
use crate::theme::ActiveTheme as _;

type SelectHandler = Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// One choice in a [`SegmentedControl`].
pub struct Segment {
    label: SharedString,
    swatch: Option<Hsla>,
}

impl Segment {
    /// A choice labelled `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            swatch: None,
        }
    }

    /// A choice shown as a colour dot (with `label` as its tooltip-free name), e.g. a colour
    /// picker.
    pub fn swatch(label: impl Into<SharedString>, colour: Hsla) -> Self {
        Self {
            label: label.into(),
            swatch: Some(colour),
        }
    }
}

/// A row of choices with one selected. Stateless: the owner passes the selected index each
/// frame and gets the new one from `on_select`, on a click or Left/Right while focused.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    segments: Vec<Segment>,
    selected: usize,
    focus_handle: FocusHandle,
    on_select: Option<SelectHandler>,
}

impl SegmentedControl {
    /// Choices `segments` with `selected` chosen, focusable through `focus_handle`.
    pub fn new(
        id: impl Into<ElementId>,
        segments: Vec<Segment>,
        selected: usize,
        focus_handle: &FocusHandle,
    ) -> Self {
        Self {
            id: id.into(),
            segments,
            selected,
            focus_handle: focus_handle.clone(),
            on_select: None,
        }
    }

    /// Called with the newly chosen index.
    #[must_use]
    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let is_focused = self.focus_handle.is_focused(window);
        let count = self.segments.len();
        let selected = self.selected;
        let handler = self.on_select;
        let step = |forward: bool| {
            let handler = handler.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(handler) = &handler
                    && count > 0
                {
                    let next = if forward {
                        (selected + 1) % count
                    } else {
                        selected.checked_sub(1).unwrap_or(count - 1)
                    };
                    handler(&next, window, cx);
                }
            }
        };
        let next = step(true);
        let previous = step(false);
        div()
            .id(self.id)
            .key_context("SegmentedControl")
            .track_focus(&self.focus_handle)
            .on_action(move |_: &SelectNext, window, cx| next(window, cx))
            .on_action(move |_: &SelectPrevious, window, cx| previous(window, cx))
            .flex()
            .flex_wrap()
            .rounded(theme.radius.sm)
            .border(theme.metrics.border_width)
            .border_color(if is_focused {
                theme.colors.accent
            } else {
                theme.colors.border
            })
            .overflow_hidden()
            .children(
                self.segments
                    .into_iter()
                    .enumerate()
                    .map(|(index, segment)| {
                        let is_selected = index == selected;
                        let handler = handler.clone();
                        let hover = theme.colors.control_hover;
                        div()
                            .id(index)
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(theme.spacing.sm)
                            .px(theme.spacing.md)
                            .py(theme.spacing.sm)
                            .cursor_pointer()
                            .when(index > 0, |d| {
                                d.border_l(theme.metrics.border_width)
                                    .border_color(theme.colors.border)
                            })
                            .when(is_selected, |d| {
                                d.bg(theme.colors.accent)
                                    .text_color(theme.colors.accent_text)
                            })
                            .when(!is_selected, |d| d.hover(move |s| s.bg(hover)))
                            .when_some(handler, |d, handler| {
                                d.on_click(move |_, window, cx| handler(&index, window, cx))
                            })
                            .map(|d| match segment.swatch {
                                Some(colour) => d.child(
                                    div()
                                        .size(theme.metrics.status_dot * 1.5)
                                        .rounded_full()
                                        .bg(colour)
                                        .when(is_selected, |d| {
                                            d.border(theme.metrics.border_width)
                                                .border_color(theme.colors.accent_text)
                                        }),
                                ),
                                None => d.child(segment.label),
                            })
                    }),
            )
    }
}
