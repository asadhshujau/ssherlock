//! A menu of actions opened at the pointer, usually by a right click.
//!
//! Each item is an action that already has its own key binding: the menu only offers another
//! way to trigger it (CLAUDE.md §3: no click-only features). Items are dispatched to the
//! focus handle of the view the menu was opened for, as if the key had been pressed there.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Action, App, Context, Corner, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, actions, anchored, deferred, div,
};

use crate::theme::ActiveTheme as _;

actions!(
    menu,
    [
        /// Highlights the next item.
        SelectNext,
        /// Highlights the previous item.
        SelectPrevious,
        /// Runs the highlighted item.
        ConfirmItem,
        /// Closes the menu.
        Dismiss,
    ]
);

/// One entry in a [`ContextMenu`].
pub struct MenuItem {
    label: SharedString,
    action: Box<dyn Action>,
    is_enabled: bool,
}

impl MenuItem {
    /// An item labelled `label` that dispatches `action`.
    pub fn new(label: impl Into<SharedString>, action: impl Action) -> Self {
        Self {
            label: label.into(),
            action: Box::new(action),
            is_enabled: true,
        }
    }

    /// Greys the item out and ignores clicks on it when `is_enabled` is false.
    #[must_use]
    pub fn enabled(mut self, is_enabled: bool) -> Self {
        self.is_enabled = is_enabled;
        self
    }
}

/// The menu closed, with or without running an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuDismissed;

/// A menu of actions shown at a point.
pub struct ContextMenu {
    items: Vec<MenuItem>,
    position: Point<Pixels>,
    /// Where the chosen action is dispatched.
    target: FocusHandle,
    selected: Option<usize>,
    focus_handle: FocusHandle,
}

impl EventEmitter<MenuDismissed> for ContextMenu {}

impl Focusable for ContextMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ContextMenu {
    /// A menu at `position` (window coordinates) whose items act on `target`. Takes focus so
    /// the keyboard works in it.
    pub fn new(
        items: Vec<MenuItem>,
        position: Point<Pixels>,
        target: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self {
            items,
            position,
            target,
            selected: None,
            focus_handle,
        }
    }

    fn run(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index).filter(|item| item.is_enabled) else {
            return;
        };
        let action = item.action.boxed_clone();
        // Focus goes back first, so the action runs in the target's key context.
        window.focus(&self.target);
        self.target.dispatch_action(&*action, window, cx);
        cx.emit(MenuDismissed);
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let enabled: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.items.get(i).is_some_and(|item| item.is_enabled))
            .collect();
        self.selected = next_enabled(&enabled, self.selected, forward);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    fn confirm_item(&mut self, _: &ConfirmItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.run(index, window, cx);
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.target);
        cx.emit(MenuDismissed);
    }
}

/// The enabled item after (or before) `current`, wrapping; the first (or last) if none is
/// highlighted yet.
fn next_enabled(enabled: &[usize], current: Option<usize>, forward: bool) -> Option<usize> {
    let position = current.and_then(|current| enabled.iter().position(|&i| i == current));
    let next = match (position, forward) {
        (None, true) => 0,
        (None, false) => enabled.len().checked_sub(1)?,
        (Some(p), true) => (p + 1) % enabled.len(),
        (Some(p), false) => p.checked_sub(1).unwrap_or(enabled.len() - 1),
    };
    enabled.get(next).copied()
}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let colors = &theme.colors;
        let items = self.items.iter().enumerate().map(|(index, item)| {
            let shortcut = window
                .highest_precedence_binding_for_action_in(&*item.action, &self.target)
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            let is_selected = self.selected == Some(index);
            let hover = colors.control_hover;
            div()
                .id(index)
                .flex()
                .justify_between()
                .gap(theme.spacing.xl)
                .px(theme.spacing.md)
                .py(theme.spacing.xs)
                .rounded(theme.radius.sm)
                .when(item.is_enabled, |d| {
                    d.text_color(colors.text)
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |menu, _, window, cx| {
                            menu.run(index, window, cx);
                        }))
                })
                .when(!item.is_enabled, |d| d.text_color(colors.text_disabled))
                .when(is_selected, |d| d.bg(colors.control_hover))
                .child(item.label.clone())
                .children(shortcut.map(|keys| div().text_color(colors.text_muted).child(keys)))
        });

        deferred(
            anchored()
                .position(self.position)
                .anchor(Corner::TopLeft)
                .snap_to_window_with_margin(theme.spacing.md)
                .child(
                    div()
                        .id("context-menu")
                        .key_context("ContextMenu")
                        .track_focus(&self.focus_handle)
                        .on_action(cx.listener(Self::select_next))
                        .on_action(cx.listener(Self::select_previous))
                        .on_action(cx.listener(Self::confirm_item))
                        .on_action(cx.listener(Self::dismiss))
                        .on_mouse_down_out(cx.listener(|menu, _, window, cx| {
                            menu.dismiss(&Dismiss, window, cx);
                        }))
                        .flex()
                        .flex_col()
                        .min_w(theme.metrics.menu_width)
                        .p(theme.spacing.xs)
                        .rounded(theme.radius.md)
                        .bg(colors.surface)
                        .border(theme.metrics.border_width)
                        .border_color(colors.border)
                        .shadow_md()
                        .text_size(theme.text.small_size)
                        .children(items),
                ),
        )
        .with_priority(1)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn keyboard_selection_skips_disabled_items_and_wraps() {
        // Items 0, 2 and 3 are enabled.
        let enabled = [0, 2, 3];
        let cases = [
            ("first down", None, true, Some(0)),
            ("first up", None, false, Some(3)),
            ("skips disabled", Some(0), true, Some(2)),
            ("wraps forward", Some(3), true, Some(0)),
            ("wraps back", Some(0), false, Some(3)),
        ];
        for (name, current, forward, expected) in cases {
            assert_eq!(next_enabled(&enabled, current, forward), expected, "{name}");
        }
        assert_eq!(next_enabled(&[], None, true), None);
    }
}
