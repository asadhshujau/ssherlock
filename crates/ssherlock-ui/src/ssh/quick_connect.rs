//! [`QuickConnectBar`]: a floating field for `user@host:port`, parsed by
//! [`ConnectTarget::parse`] (CONN-23). It reports a target or a dismissal; the pane opens the tab.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div,
};
use ssherlock_core::ConnectTarget;

use super::status::capitalise;
use crate::components::{InputKind, TextInput, TextInputEvent};
use crate::theme::ActiveTheme as _;

/// What the bar reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickConnectEvent {
    /// Enter on valid text.
    Connect(ConnectTarget),
    /// Escape.
    Dismissed,
}

/// The quick connect bar.
pub struct QuickConnectBar {
    input: Entity<TextInput>,
    error: Option<SharedString>,
    _subscription: Subscription,
}

impl EventEmitter<QuickConnectEvent> for QuickConnectBar {}

impl Focusable for QuickConnectBar {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl QuickConnectBar {
    /// An empty, focused bar.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("", InputKind::Persistent, window, cx));
        let subscription = cx.subscribe_in(&input, window, Self::input_event);
        Self {
            input,
            error: None,
            _subscription: subscription,
        }
    }

    fn input_event(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed(_) => {
                if self.error.take().is_some() {
                    cx.notify();
                }
            }
            TextInputEvent::Confirmed(text) => match ConnectTarget::parse(text) {
                Ok(target) => cx.emit(QuickConnectEvent::Connect(target)),
                Err(error) => {
                    self.error = Some(capitalise(&error.to_string()).into());
                    cx.notify();
                }
            },
            TextInputEvent::Cancelled => cx.emit(QuickConnectEvent::Dismissed),
        }
    }
}

impl Render for QuickConnectBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .key_context("QuickConnect")
            .absolute()
            .top(theme.spacing.xl)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.sm)
                    .w(theme.metrics.modal_width)
                    .p(theme.spacing.md)
                    .rounded(theme.radius.md)
                    .bg(theme.colors.surface)
                    .border(theme.metrics.border_width)
                    .border_color(theme.colors.accent)
                    .text_size(theme.text.ui_size)
                    .child(
                        div()
                            .px(theme.spacing.sm)
                            .py(theme.spacing.xs)
                            .rounded(theme.radius.sm)
                            .bg(theme.colors.background)
                            .child(self.input.clone()),
                    )
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .when_some(self.error.clone(), |d, error| d.text_color(theme.colors.text).child(error))
                            .when(self.error.is_none(), |d| {
                                d.child("user@host:port or ssh://user@host \u{2014} Enter to connect, Esc to cancel")
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;

    use super::*;

    fn events_after(
        typed: &str,
        cx: &mut TestAppContext,
    ) -> (Vec<QuickConnectEvent>, Option<SharedString>) {
        cx.update(crate::keymap::init);
        let (bar, cx) = cx.add_window_view(QuickConnectBar::new);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&bar, move |_, event: &QuickConnectEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        cx.simulate_input(typed);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let error = bar.read_with(cx, |bar, _| bar.error.clone());
        (seen.take(), error)
    }

    #[gpui::test]
    fn valid_text_connects(cx: &mut TestAppContext) {
        let (events, error) = events_after("alice@example.com:2222", cx);
        assert_eq!(
            events,
            [QuickConnectEvent::Connect(
                ConnectTarget::parse("alice@example.com:2222").unwrap()
            )]
        );
        assert_eq!(error, None);
    }

    #[gpui::test]
    fn invalid_text_shows_why(cx: &mut TestAppContext) {
        let (events, error) = events_after("example.com:99999", cx);
        assert_eq!(events, []);
        assert_eq!(
            error.as_ref().map(AsRef::as_ref),
            Some("The port \"99999\" is not a number from 1 to 65535")
        );
    }
}
