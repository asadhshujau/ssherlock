//! [`SessionPrompts`]: the questions a connection asks the user, shown as dialogs. Whether the
//! server's host key should be trusted, and what to type to log in (a password, a passphrase,
//! a 2FA code).
//!
//! Every connection asks the same questions the same way: a terminal tab, and a
//! forwarding-only connection started from Port forwarding. The words come from
//! [`dialogs`](super::dialogs); this entity turns them into [`Modal`]s and sends the answers
//! back to the waiting session. Its owner draws it over itself while [`is_open`] says so.
//!
//! [`is_open`]: SessionPrompts::is_open

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};
use ssherlock_core::SecretString;
use ssherlock_ssh::SessionEvent;
use ssherlock_ssh::auth::AuthPrompt;
use ssherlock_ssh::known_hosts::randomart;
use ssherlock_ssh::session::{HostKeyDecision, HostKeyPrompt};

use super::dialogs::{KeyRow, host_key_text, login_text};
use crate::components::{InputKind, Modal, ModalEvent, TextInput, TextInputEvent};
use crate::terminal::mono_family;
use crate::theme::ActiveTheme as _;

/// What [`SessionPrompts`] tells its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPromptsEvent {
    /// The dialog closed. `had_focus` says whether it was focused, so the owner can take focus
    /// back.
    Closed {
        /// Whether focus was inside the dialog.
        had_focus: bool,
    },
}

/// A question the session is waiting on.
enum Dialog {
    HostKey {
        prompt: Option<HostKeyPrompt>,
        modal: Entity<Modal>,
        _subscription: Subscription,
    },
    Login {
        prompt: Option<AuthPrompt>,
        modal: Entity<Modal>,
        inputs: Vec<(Entity<TextInput>, bool)>,
        _subscriptions: Vec<Subscription>,
    },
}

impl Dialog {
    fn modal(&self) -> &Entity<Modal> {
        match self {
            Self::HostKey { modal, .. } | Self::Login { modal, .. } => modal,
        }
    }
}

/// The host key and login dialogs of one connection.
pub struct SessionPrompts {
    dialog: Option<Dialog>,
    /// Focused when there's no dialog; nothing ever focuses it in practice.
    idle_focus: FocusHandle,
}

impl EventEmitter<SessionPromptsEvent> for SessionPrompts {}

impl Focusable for SessionPrompts {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.dialog {
            Some(Dialog::Login { inputs, .. }) if !inputs.is_empty() => inputs.first().map_or_else(
                || self.idle_focus.clone(),
                |(input, _)| input.focus_handle(cx),
            ),
            Some(dialog) => dialog.modal().focus_handle(cx),
            None => self.idle_focus.clone(),
        }
    }
}

impl SessionPrompts {
    /// No dialog yet.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            dialog: None,
            idle_focus: cx.focus_handle(),
        }
    }

    /// Whether a dialog is showing.
    pub fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Whether focus is inside the dialog.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.modal().focus_handle(cx).contains_focused(window, cx))
    }

    /// Shows the question `event` asks, if it is one, and returns `None`; any other event is
    /// handed back. A new question replaces one still showing, which is then cancelled.
    pub fn ask(
        &mut self,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SessionEvent> {
        match event {
            SessionEvent::HostKey(prompt) => self.ask_host_key(prompt, window, cx),
            SessionEvent::Prompt(prompt) => self.ask_login(prompt, window, cx),
            other => return Some(other),
        }
        cx.notify();
        None
    }

    fn ask_host_key(&mut self, prompt: HostKeyPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let text = host_key_text(prompt.host(), prompt.key(), prompt.concern());
        let art = randomart(prompt.key());
        let rows = text.rows.clone();
        let modal = cx.new(|cx| {
            let modal = Modal::new(text.title, text.message, text.confirm, window, cx)
                .detail(move |window, cx| host_key_detail(&rows, &art, window, cx));
            if text.is_danger {
                modal.danger()
            } else {
                modal
            }
        });
        let subscription =
            cx.subscribe_in(&modal, window, |this, _, event: &ModalEvent, window, cx| {
                if let Some(Dialog::HostKey { prompt, .. }) = &mut this.dialog
                    && let Some(prompt) = prompt.take()
                {
                    let decision = match event {
                        ModalEvent::Confirmed => HostKeyDecision::Trust,
                        ModalEvent::Cancelled => HostKeyDecision::Reject,
                    };
                    prompt.answer(decision);
                }
                this.close(window, cx);
            });
        self.dialog = Some(Dialog::HostKey {
            prompt: Some(prompt),
            modal,
            _subscription: subscription,
        });
    }

    fn ask_login(&mut self, prompt: AuthPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let (title, message, confirm) = login_text(prompt.kind());
        let inputs: Vec<(Entity<TextInput>, bool)> = prompt
            .fields()
            .iter()
            .map(|field| {
                let is_secret = !field.echo;
                let input = cx.new(|cx| {
                    let input = TextInput::new("", InputKind::Persistent, window, cx);
                    if is_secret { input.secret() } else { input }
                });
                (input, is_secret)
            })
            .collect();
        let fields: Vec<(SharedString, Entity<TextInput>)> = prompt
            .fields()
            .iter()
            .map(|field| SharedString::from(field.label.trim().to_owned()))
            .zip(inputs.iter().map(|(input, _)| input.clone()))
            .collect();
        let modal = cx.new(|cx| {
            Modal::new(title, message, confirm, window, cx)
                .detail(move |_, cx| login_fields(&fields, cx))
        });
        // The modal took focus as it opened; typing belongs in the first field.
        if let Some((first, _)) = inputs.first() {
            first.focus_handle(cx).focus(window);
        }

        let mut subscriptions =
            vec![
                cx.subscribe_in(&modal, window, |this, _, event: &ModalEvent, window, cx| {
                    match event {
                        ModalEvent::Confirmed => this.submit_login(window, cx),
                        ModalEvent::Cancelled => this.close(window, cx),
                    }
                }),
            ];
        for (index, (input, _)) in inputs.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed(_) => {}
                    TextInputEvent::Confirmed(_) => this.next_field_or_submit(index, window, cx),
                    TextInputEvent::Cancelled => this.close(window, cx),
                },
            ));
        }
        self.dialog = Some(Dialog::Login {
            prompt: Some(prompt),
            modal,
            inputs,
            _subscriptions: subscriptions,
        });
    }

    fn next_field_or_submit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let next = match &self.dialog {
            Some(Dialog::Login { inputs, .. }) => {
                inputs.get(index + 1).map(|(input, _)| input.clone())
            }
            _ => return,
        };
        match next {
            Some(input) => input.focus_handle(cx).focus(window),
            None => self.submit_login(window, cx),
        }
    }

    fn submit_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Dialog::Login { prompt, inputs, .. }) = &mut self.dialog
            && let Some(prompt) = prompt.take()
        {
            let answers: Vec<SecretString> = inputs
                .iter()
                .map(|(input, is_secret)| {
                    input.update(cx, |input, cx| {
                        if *is_secret {
                            input.take_secret(cx)
                        } else {
                            SecretString::from(input.text())
                        }
                    })
                })
                .collect();
            prompt.answer(answers);
        }
        self.close(window, cx);
    }

    /// Drops the dialog. A question still inside is dropped unanswered, which cancels it.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.contains_focus(window, cx);
        if self.dialog.take().is_some() {
            cx.emit(SessionPromptsEvent::Closed { had_focus });
            cx.notify();
        }
    }
}

impl Render for SessionPrompts {
    /// The modal itself, with nothing around it: it fills the nearest box, which must be the
    /// owner's, so the owner draws this as its own child.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match &self.dialog {
            Some(dialog) => dialog.modal().clone().into_any_element(),
            None => div().into_any_element(),
        }
    }
}

fn host_key_detail(rows: &[KeyRow], art: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let family = mono_family(&theme, window, cx);
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.md)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .children(rows.iter().map(|row| {
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(theme.text.small_size)
                                .text_color(theme.colors.text_muted)
                                .child(row.label.clone()),
                        )
                        .child(
                            div()
                                .font_family(family.clone())
                                .text_size(theme.text.small_size)
                                .child(row.value.clone()),
                        )
                        .when_some(row.note.clone(), |d, note| {
                            d.child(
                                div()
                                    .text_size(theme.text.small_size)
                                    .text_color(theme.colors.text_muted)
                                    .child(format!("Trusted in {note}")),
                            )
                        })
                })),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .p(theme.spacing.md)
                .rounded(theme.radius.sm)
                .bg(theme.terminal.background)
                .text_color(theme.terminal.foreground)
                .font_family(family)
                .text_size(theme.text.small_size)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .children(art.lines().map(str::to_owned)),
                ),
        )
        .into_any_element()
}

fn login_fields(fields: &[(SharedString, Entity<TextInput>)], cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.md)
        .children(fields.iter().map(|(label, input)| {
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .when(!label.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(label.clone()),
                    )
                })
                .child(
                    div()
                        .px(theme.spacing.sm)
                        .py(theme.spacing.xs)
                        .rounded(theme.radius.sm)
                        .bg(theme.colors.background)
                        .border(theme.metrics.border_width)
                        .border_color(theme.colors.border)
                        .child(input.clone()),
                )
        }))
        .into_any_element()
}
