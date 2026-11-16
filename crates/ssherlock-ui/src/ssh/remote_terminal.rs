//! [`RemoteTerminal`]: a terminal running a shell on an SSH server.
//!
//! It wraps an ordinary [`TerminalView`] fed by [`spawn_shell`], and adds what only a remote
//! terminal needs: the host key and login dialogs the session asks for, and the status bar. It
//! re-emits the terminal's [`TerminalViewEvent`]s, so the pane treats it like any terminal.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    div,
};
use ssherlock_core::{ConnectTarget, SecretString};
use ssherlock_ssh::auth::{AuthMethod, AuthPrompt, default_key_files};
use ssherlock_ssh::known_hosts::{HostHashing, randomart};
use ssherlock_ssh::session::{ConnectConfig, DEFAULT_PORT, HostKeyDecision, HostKeyPrompt};
use ssherlock_ssh::{ConnectionState, HostKeys, RemoteShellConfig, SessionEvent, spawn_shell};
use ssherlock_terminal::{PtyExit, PtyIo, PtyOutput, TerminalSize};
use tokio::sync::mpsc;

use super::dialogs::{KeyRow, host_key_text, login_text};
use super::{SshEnvironment, status};
use crate::components::{InputKind, Modal, ModalEvent, TextInput, TextInputEvent};
use crate::runtime::TokioHandle;
use crate::terminal::{TerminalView, TerminalViewEvent, mono_family};
use crate::theme::ActiveTheme as _;

/// Session events in flight to the view: a few state changes and at most one question at a
/// time (the session waits for each answer), so a small buffer never fills in practice.
const EVENT_CAPACITY: usize = 16;

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
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::HostKey { modal, .. } => modal.focus_handle(cx),
            Self::Login { modal, inputs, .. } => inputs.first().map_or_else(
                || modal.focus_handle(cx),
                |(input, _)| input.focus_handle(cx),
            ),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Self::HostKey { modal, .. } | Self::Login { modal, .. } => {
                modal.clone().into_any_element()
            }
        }
    }
}

/// A terminal running a shell on an SSH server.
pub struct RemoteTerminal {
    /// `user@host[:port]`, shown in the status bar and as the tab's fallback title.
    label: SharedString,
    terminal: Entity<TerminalView>,
    state: Option<ConnectionState>,
    dialog: Option<Dialog>,
    _subscriptions: Vec<Subscription>,
    _events: Option<Task<()>>,
}

impl EventEmitter<TerminalViewEvent> for RemoteTerminal {}

impl Focusable for RemoteTerminal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.dialog.as_ref().map_or_else(
            || self.terminal.focus_handle(cx),
            |dialog| dialog.focus_handle(cx),
        )
    }
}

impl RemoteTerminal {
    /// Connects to `target` and shows its shell. Missing parts come from the
    /// [`SshEnvironment`]: the local user name, port 22, the default key files.
    pub fn new(target: &ConnectTarget, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let env = cx.try_global::<SshEnvironment>().cloned();
        let tokio = cx.try_global::<TokioHandle>().map(|t| t.handle().clone());
        let user = target
            .user
            .clone()
            .or_else(|| env.as_ref().and_then(|env| env.local_user.clone()));
        let port = target.port.map_or(DEFAULT_PORT, ssherlock_core::Port::get);
        let label = match &user {
            Some(user) => format!("{user}@{}", host_label(target, port)),
            None => host_label(target, port),
        };
        let size = TerminalSize::default();

        let (pty, events, failure) = match (env, tokio, user) {
            (Some(env), Some(tokio), Some(user)) => {
                let connect = ConnectConfig::new(target.address.to_string(), user).with_port(port);
                let key_files = env
                    .ssh_dir
                    .as_deref()
                    .map(default_key_files)
                    .unwrap_or_default();
                let config = RemoteShellConfig::new(connect, AuthMethod::defaults(key_files));
                let host_keys = HostKeys::new(
                    env.known_hosts,
                    env.read_only_known_hosts,
                    HostHashing::Hashed,
                );
                let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
                (
                    spawn_shell(config, host_keys, size, tx, &tokio),
                    Some(rx),
                    None,
                )
            }
            (None, _, _) => (
                finished_pty(),
                None,
                Some("SSH is not set up: there is no data directory"),
            ),
            (_, None, _) => (
                finished_pty(),
                None,
                Some("the app's async runtime isn't running"),
            ),
            (_, _, None) => (
                finished_pty(),
                None,
                Some("no user name: connect as user@host"),
            ),
        };

        let terminal = cx.new(|cx| {
            let mut view = TerminalView::with_pty(pty, size, window, cx);
            // The status bar says how the connection ended, in more words.
            view.set_shows_exit_banner(false, cx);
            view
        });
        let forward = cx.subscribe(&terminal, |_, _, event: &TerminalViewEvent, cx| {
            cx.emit(event.clone());
        });
        let events = events.map(|mut rx| {
            cx.spawn_in(window, async move |this, cx| {
                while let Some(event) = rx.recv().await {
                    if this
                        .update_in(cx, |view, window, cx| view.session_event(event, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let state = failure.map(|message| {
            ConnectionState::Disconnected(ssherlock_ssh::connection::DisconnectReason::Failed(
                message.to_owned(),
            ))
        });
        Self {
            label: label.into(),
            terminal,
            state,
            dialog: None,
            _subscriptions: vec![forward],
            _events: events,
        }
    }

    /// The title the remote program set, if any.
    pub fn title(&self, cx: &App) -> Option<SharedString> {
        self.terminal.read(cx).title().cloned()
    }

    /// `user@host[:port]`.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The connection's latest state.
    pub fn state(&self) -> Option<&ConnectionState> {
        self.state.as_ref()
    }

    fn session_event(&mut self, event: SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::State(state) => self.state = Some(state),
            SessionEvent::HostKey(prompt) => self.ask_host_key(prompt, window, cx),
            SessionEvent::Prompt(prompt) => self.ask_login(prompt, window, cx),
            SessionEvent::Banner(banner) => {
                // Shown by the server before login; the shell shows its own MOTD afterwards.
                tracing::info!(host = %self.label, banner = banner.trim(), "server banner");
            }
        }
        cx.notify();
    }

    fn ask_host_key(&mut self, prompt: HostKeyPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let text = host_key_text(prompt.host(), prompt.key(), prompt.concern());
        let art = randomart(prompt.key());
        let rows = text.rows.clone();
        let previous_focus = self.focus_before_dialog(window, cx);
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
            cx.subscribe_in(&modal, window, |view, _, event: &ModalEvent, window, cx| {
                if let Some(Dialog::HostKey { prompt, .. }) = &mut view.dialog
                    && let Some(prompt) = prompt.take()
                {
                    let decision = match event {
                        ModalEvent::Confirmed => HostKeyDecision::Trust,
                        ModalEvent::Cancelled => HostKeyDecision::Reject,
                    };
                    prompt.answer(decision);
                }
                view.close_dialog(window, cx);
            });
        self.dialog = Some(Dialog::HostKey {
            prompt: Some(prompt),
            modal,
            _subscription: subscription,
        });
        Self::restore_focus(previous_focus, window, cx);
    }

    fn ask_login(&mut self, prompt: AuthPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let (title, message, confirm) = login_text(prompt.kind());
        let previous_focus = self.focus_before_dialog(window, cx);
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
                cx.subscribe_in(&modal, window, |view, _, event: &ModalEvent, window, cx| {
                    match event {
                        ModalEvent::Confirmed => view.submit_login(window, cx),
                        ModalEvent::Cancelled => view.close_dialog(window, cx),
                    }
                }),
            ];
        for (index, (input, _)) in inputs.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |view, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed(_) => {}
                    TextInputEvent::Confirmed(_) => view.next_field_or_submit(index, window, cx),
                    TextInputEvent::Cancelled => view.close_dialog(window, cx),
                },
            ));
        }
        self.dialog = Some(Dialog::Login {
            prompt: Some(prompt),
            modal,
            inputs,
            _subscriptions: subscriptions,
        });
        Self::restore_focus(previous_focus, window, cx);
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
        self.close_dialog(window, cx);
    }

    /// Drops the dialog. A prompt still inside is dropped unanswered, which cancels it.
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The fields sit inside the modal, so its handle covers them too.
        let had_focus = self.dialog.as_ref().is_some_and(|dialog| match dialog {
            Dialog::HostKey { modal, .. } | Dialog::Login { modal, .. } => {
                modal.focus_handle(cx).contains_focused(window, cx)
            }
        });
        self.dialog = None;
        if had_focus {
            self.terminal.focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    /// Where focus was before a dialog opened, if it wasn't in this terminal. A dialog takes
    /// focus as it opens; for a tab in the background, focus goes back to where the user is.
    fn focus_before_dialog(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        let ours = self.focus_handle(cx);
        window
            .focused(cx)
            .filter(|focused| *focused != ours && !ours.contains_focused(window, cx))
    }

    fn restore_focus(previous: Option<FocusHandle>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = previous {
            previous.focus(window);
        }
        cx.notify();
    }
}

/// `host` or `host:port` (IPv6 bracketed) for labels, leaving out the default port.
fn host_label(target: &ConnectTarget, port: u16) -> String {
    if port == DEFAULT_PORT {
        target.address.to_string()
    } else {
        format!("{}:{port}", target.address.to_bracketed())
    }
}

/// A pty whose program has already ended, for a terminal that couldn't connect at all.
fn finished_pty() -> PtyIo {
    let (io, peer) = PtyIo::pair();
    // Fresh channel with free capacity, so this can't fail; if it did, the terminal would
    // simply show nothing.
    let _ = peer.output.try_send(PtyOutput::Exited(PtyExit::Unknown));
    io
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

impl Render for RemoteTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let status = self.state.as_ref().and_then(status::describe);
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .child(div().flex().flex_1().min_h_0().child(self.terminal.clone()))
            .when_some(status, |d, (tone, text)| {
                d.child(status::bar(tone, text, self.label.clone(), &theme))
            })
            .when_some(self.dialog.as_ref(), |d, dialog| d.child(dialog.element()))
    }
}
