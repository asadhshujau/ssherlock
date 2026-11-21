//! [`RemoteTerminal`]: a terminal running a shell on an SSH server.
//!
//! It wraps an ordinary [`TerminalView`] fed by [`spawn_shell`], and adds what only a remote
//! terminal needs: the host key and login dialogs the session asks for ([`SessionPrompts`]),
//! and the status bar. It re-emits the terminal's [`TerminalViewEvent`]s, so the pane treats
//! it like any terminal.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div,
};
use ssherlock_core::ConnectTarget;
use ssherlock_core::keychain::Key;
use ssherlock_ssh::{ConnectionHandle, ConnectionState, KeyInstall, SessionEvent, spawn_shell};
use ssherlock_terminal::{PtyExit, PtyIo, PtyOutput, TerminalSize};
use tokio::sync::mpsc;

use super::actions::ExportKeyHere;
use super::prompts::{SessionPrompts, SessionPromptsEvent};
use super::{ConnectOptions, EVENT_CAPACITY, prepare_connection, status};
use crate::components::{Picker, PickerEvent, PickerItem};
use crate::hosts::HostsModel;
use crate::notify;
use crate::terminal::{TerminalView, TerminalViewEvent};
use crate::theme::ActiveTheme as _;

/// A terminal running a shell on an SSH server.
pub struct RemoteTerminal {
    /// `user@host[:port]`, shown in the status bar and as the tab's fallback title.
    label: SharedString,
    terminal: Entity<TerminalView>,
    state: Option<ConnectionState>,
    prompts: Entity<SessionPrompts>,
    /// Choosing a key to add to the server's `authorized_keys`.
    key_picker: Option<(Entity<Picker>, Subscription)>,
    /// Asks the connection for work beside the shell; `None` if it never started.
    connection: Option<ConnectionHandle>,
    _subscriptions: Vec<Subscription>,
    _events: Option<Task<()>>,
}

impl EventEmitter<TerminalViewEvent> for RemoteTerminal {}

impl Focusable for RemoteTerminal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if let Some((picker, _)) = &self.key_picker {
            return picker.focus_handle(cx);
        }
        let prompts = self.prompts.read(cx);
        if prompts.is_open() {
            prompts.focus_handle(cx)
        } else {
            self.terminal.focus_handle(cx)
        }
    }
}

impl RemoteTerminal {
    /// Connects to `target` with `options` and shows its shell. Missing parts come from the
    /// [`SshEnvironment`](super::SshEnvironment): the local user name, port 22, the default key
    /// files.
    pub fn new(
        target: &ConnectTarget,
        options: &ConnectOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prepared = prepare_connection(target, options, cx);
        let size = TerminalSize::default();
        let (pty, connection, events, failure) = match prepared.setup {
            Ok(setup) => {
                let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
                let (pty, connection) =
                    spawn_shell(setup.config, setup.host_keys, size, tx, &setup.runtime);
                (pty, Some(connection), Some(rx), None)
            }
            Err(reason) => (finished_pty(), None, None, Some(reason)),
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
        let prompts = cx.new(SessionPrompts::new);
        let closed = cx.subscribe_in(
            &prompts,
            window,
            |view, _, event: &SessionPromptsEvent, window, cx| {
                let SessionPromptsEvent::Closed { had_focus } = event;
                if *had_focus {
                    view.terminal.focus_handle(cx).focus(window);
                }
                cx.notify();
            },
        );
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
            label: prepared.label,
            terminal,
            state,
            prompts,
            key_picker: None,
            connection,
            _subscriptions: vec![forward, closed],
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

    /// The tab's connection, for work beside the shell such as port forwards; `None` if it
    /// never started.
    pub fn connection(&self) -> Option<&ConnectionHandle> {
        self.connection.as_ref()
    }

    fn session_event(&mut self, event: SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::State(state) => self.state = Some(state),
            SessionEvent::Banner(banner) => {
                // Shown by the server before login; the shell shows its own MOTD afterwards.
                tracing::info!(host = %self.label, banner = banner.trim(), "server banner");
            }
            question @ (SessionEvent::HostKey(_) | SessionEvent::Prompt(_)) => {
                let previous = self.focus_before_dialog(window, cx);
                self.prompts.update(cx, |prompts, cx| {
                    // Always a question, so nothing comes back.
                    let _ = prompts.ask(question, window, cx);
                });
                if let Some(previous) = previous {
                    previous.focus(window);
                }
            }
        }
        cx.notify();
    }

    /// Adds `key` to the server's `authorized_keys` over this tab's connection (KEY-07), once
    /// it's connected, and says how it went.
    pub fn install_key(&mut self, key: &Key, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            notify::info(format!("{} isn't connected", self.label), cx);
            return;
        };
        let public_key = key.public_key.clone();
        let key_title = key.title().to_owned();
        let host = self.label.clone();
        cx.spawn(async move |_, cx| {
            let outcome = connection.install_public_key(&public_key).await;
            let _ = cx.update(|cx| match outcome {
                Ok(KeyInstall::Added) => {
                    notify::info(format!("Added {key_title} to {host}'s authorized_keys"), cx);
                }
                // Starts with a fixed word: toasts capitalise the first letter, and a user name
                // mustn't change case.
                Ok(KeyInstall::AlreadyPresent) => {
                    notify::info(
                        format!("Nothing to add: {host} already accepts {key_title}"),
                        cx,
                    );
                }
                Err(error) => notify::error(&error, cx),
            });
        })
        .detach();
    }

    fn export_key_here(&mut self, _: &ExportKeyHere, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = HostsModel::global(cx) else {
            return;
        };
        let keys: Vec<Key> = crate::keychain::list::sorted(model.read(cx).keys())
            .into_iter()
            .cloned()
            .collect();
        if keys.is_empty() {
            notify::info(
                "The keychain has no keys yet; generate or import one first",
                cx,
            );
            return;
        }
        let items = keys
            .iter()
            .map(|k| PickerItem::new(k.title().to_owned()).detail(k.kind.display_name()))
            .collect();
        let title = format!("Add a key to {}'s authorized_keys", self.label);
        let picker = cx.new(|cx| Picker::new(title, items, window, cx));
        let subscription = cx.subscribe_in(
            &picker,
            window,
            move |view, _, event: &PickerEvent, window, cx| {
                if let PickerEvent::Picked(index) = event
                    && let Some(key) = keys.get(*index)
                {
                    view.install_key(key, cx);
                }
                view.close_key_picker(window, cx);
            },
        );
        self.key_picker = Some((picker, subscription));
        cx.notify();
    }

    fn close_key_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self
            .key_picker
            .as_ref()
            .is_some_and(|(picker, _)| picker.focus_handle(cx).contains_focused(window, cx));
        self.key_picker = None;
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
}

/// A pty whose program has already ended, for a terminal that couldn't connect at all.
fn finished_pty() -> PtyIo {
    let (io, peer) = PtyIo::pair();
    // Fresh channel with free capacity, so this can't fail; if it did, the terminal would
    // simply show nothing.
    let _ = peer.output.try_send(PtyOutput::Exited(PtyExit::Unknown));
    io
}

impl Render for RemoteTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let status = self.state.as_ref().and_then(status::describe);
        let has_prompt = self.prompts.read(cx).is_open();
        div()
            .key_context("RemoteTerminal")
            .on_action(cx.listener(Self::export_key_here))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .child(div().flex().flex_1().min_h_0().child(self.terminal.clone()))
            .when_some(status, |d, (tone, text)| {
                d.child(status::bar(tone, text, self.label.clone(), &theme))
            })
            .when(has_prompt, |d| d.child(self.prompts.clone()))
            .when_some(self.key_picker.as_ref(), |d, (picker, _)| {
                d.child(picker.clone())
            })
    }
}
