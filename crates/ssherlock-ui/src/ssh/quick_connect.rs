//! [`QuickConnectBar`]: a floating field for `user@host:port`, parsed by
//! [`ConnectTarget::parse`], that also suggests saved hosts matching what's typed (CONN-23). It
//! reports a choice or a dismissal; the pane opens the tab.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::hosts::{Host, search};
use ssherlock_core::{ConnectTarget, Id};

use super::SshEnvironment;
use super::status::capitalise;
use crate::components::context_menu::{SelectNext, SelectPrevious};
use crate::components::{InputKind, TextInput, TextInputEvent};
use crate::hosts::connect::target_text;
use crate::hosts::{ConnectRequest, HostsModel};
use crate::theme::ActiveTheme as _;

/// How many saved hosts the bar suggests at most.
const MAX_SUGGESTIONS: usize = 6;

/// What the bar reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickConnectEvent {
    /// Connect to what was typed.
    Connect(ConnectTarget),
    /// Connect to a saved host that matched.
    ConnectSaved(ConnectRequest),
    /// Escape.
    Dismissed,
}

/// One line under the field.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Choice {
    /// What was typed, if it's a valid target.
    Typed(ConnectTarget),
    /// A saved host whose title, address, tags or notes match.
    Saved {
        host: Id<Host>,
        title: SharedString,
        target: SharedString,
    },
}

/// The quick connect bar.
pub struct QuickConnectBar {
    input: Entity<TextInput>,
    error: Option<SharedString>,
    choices: Vec<Choice>,
    selected: usize,
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
            choices: Vec::new(),
            selected: 0,
            _subscription: subscription,
        }
    }

    /// The typed target (if valid) followed by matching saved hosts. The best saved match is
    /// selected when there is one: a label like `web` also parses as a host name, and the saved
    /// host is almost always what's meant. The typed target is one Up away.
    fn update_choices(&mut self, text: &str, cx: &mut Context<Self>) {
        let typed = ConnectTarget::parse(text).ok().map(Choice::Typed);
        let local_user = cx
            .try_global::<SshEnvironment>()
            .and_then(|env| env.local_user.clone());
        let saved: Vec<Choice> = HostsModel::global(cx)
            .map(|model| {
                let model = model.read(cx);
                search::rank(model.hosts(), text)
                    .into_iter()
                    .take(MAX_SUGGESTIONS)
                    .map(|host| {
                        let request = ConnectRequest::new(host, model.groups(), model.identities());
                        Choice::Saved {
                            host: host.id,
                            title: host.title().into_owned().into(),
                            target: target_text(&request.target, local_user.as_deref()).into(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.selected = usize::from(typed.is_some() && !saved.is_empty());
        self.choices = typed.into_iter().chain(saved).collect();
    }

    fn input_event(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed(text) => {
                self.error = None;
                self.update_choices(text, cx);
                cx.notify();
            }
            TextInputEvent::Confirmed(text) => self.confirm(text, cx),
            TextInputEvent::Cancelled => cx.emit(QuickConnectEvent::Dismissed),
        }
    }

    fn confirm(&mut self, text: &str, cx: &mut Context<Self>) {
        match self.choices.get(self.selected).cloned() {
            Some(Choice::Typed(target)) => cx.emit(QuickConnectEvent::Connect(target)),
            Some(Choice::Saved { host, .. }) => {
                let request = HostsModel::global(cx).and_then(|model| {
                    let model = model.read(cx);
                    model
                        .host(host)
                        .map(|h| ConnectRequest::new(h, model.groups(), model.identities()))
                });
                if let Some(request) = request {
                    cx.emit(QuickConnectEvent::ConnectSaved(request));
                }
            }
            None => {
                if let Err(error) = ConnectTarget::parse(text) {
                    self.error = Some(capitalise(&error.to_string()).into());
                    cx.notify();
                }
            }
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.choices.len();
        if count > 0 {
            self.selected = if forward {
                (self.selected + 1) % count
            } else {
                self.selected.checked_sub(1).unwrap_or(count - 1)
            };
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }
}

impl Render for QuickConnectBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.choices.iter().enumerate().map(|(index, choice)| {
            let (title, detail): (SharedString, Option<SharedString>) = match choice {
                Choice::Typed(target) => (format!("Connect to {target}").into(), None),
                Choice::Saved { title, target, .. } => (title.clone(), Some(target.clone())),
            };
            let is_selected = index == self.selected;
            let hover = theme.colors.control_hover;
            div()
                .id(("quick-connect-choice", index))
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .px(theme.spacing.sm)
                .py(theme.spacing.xs)
                .rounded(theme.radius.sm)
                .cursor_pointer()
                .when(is_selected, |d| d.bg(theme.colors.row_selected))
                .when(!is_selected, |d| d.hover(move |s| s.bg(hover)))
                .on_click(cx.listener(move |bar, _, _, cx| {
                    bar.selected = index;
                    let text = bar.input.read(cx).text().to_owned();
                    bar.confirm(&text, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(title),
                )
                .when_some(detail, |d, detail| {
                    d.child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(detail),
                    )
                })
        });
        div()
            .key_context("QuickConnect")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .absolute()
            .top(theme.spacing.xl)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    // It floats over a terminal, which takes focus on any mouse-down it sees.
                    .occlude()
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
                    .text_color(theme.colors.text)
                    .child(
                        div()
                            .debug_selector(|| "quick-connect-input".into())
                            .px(theme.spacing.sm)
                            .py(theme.spacing.xs)
                            .rounded(theme.radius.sm)
                            .bg(theme.colors.background)
                            .child(self.input.clone()),
                    )
                    .children(rows)
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .when_some(self.error.clone(), |d, error| d.text_color(theme.colors.text).child(error))
                            .when(self.error.is_none(), |d| {
                                d.child("user@host:port, ssh://user@host or a saved host \u{2014} \u{2191}\u{2193} to choose, Enter to connect, Esc to cancel")
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
    use std::sync::Arc;

    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;
    use ssherlock_core::Timestamp;
    use ssherlock_store::{Repository as _, Store};

    use super::*;

    fn events_after(
        typed: &str,
        keys: &str,
        hosts: &[Host],
        cx: &mut TestAppContext,
    ) -> (Vec<QuickConnectEvent>, Option<SharedString>) {
        cx.update(|cx| {
            crate::keymap::init(cx);
            crate::notify::init(cx);
        });
        if !hosts.is_empty() {
            let store = Arc::new(Store::open_in_memory().unwrap());
            store.upsert_all(hosts).unwrap();
            cx.update(|cx| HostsModel::install(store, cx));
            cx.run_until_parked();
        }
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
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        let error = bar.read_with(cx, |bar, _| bar.error.clone());
        (seen.take(), error)
    }

    fn saved(label: &str, address: &str) -> Host {
        let mut host = Host::new(address.parse().unwrap(), Timestamp::UNIX_EPOCH);
        host.label = label.into();
        host
    }

    #[gpui::test]
    fn valid_text_connects(cx: &mut TestAppContext) {
        let (events, error) = events_after("alice@example.com:2222", "enter", &[], cx);
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
        let (events, error) = events_after("example.com:99999", "enter", &[], cx);
        assert_eq!(events, []);
        assert_eq!(
            error.as_ref().map(AsRef::as_ref),
            Some("The port \"99999\" is not a number from 1 to 65535")
        );
    }

    #[gpui::test]
    fn a_matching_saved_host_is_chosen_by_default(cx: &mut TestAppContext) {
        let web = saved("web-1", "web1.example.com");
        let id = web.id;
        let (events, _) = events_after("web", "enter", &[web, saved("db", "db.example.com")], cx);
        let [QuickConnectEvent::ConnectSaved(request)] = events.as_slice() else {
            unreachable!("one saved-host connect, got {events:?}")
        };
        assert_eq!(request.host, id);
    }

    #[gpui::test]
    fn up_picks_what_was_typed_instead(cx: &mut TestAppContext) {
        let (events, _) =
            events_after("web", "up enter", &[saved("web-1", "web1.example.com")], cx);
        assert_eq!(
            events,
            [QuickConnectEvent::Connect(
                ConnectTarget::parse("web").unwrap()
            )]
        );
    }

    #[gpui::test]
    fn invalid_text_can_still_pick_a_saved_host(cx: &mut TestAppContext) {
        // "pg db" isn't a valid address, but it fuzzily matches the saved host.
        let (events, _) = events_after("pg db", "enter", &[saved("pg-db", "192.0.2.10")], cx);
        assert!(
            matches!(events.as_slice(), [QuickConnectEvent::ConnectSaved(_)]),
            "{events:?}"
        );
    }
}
