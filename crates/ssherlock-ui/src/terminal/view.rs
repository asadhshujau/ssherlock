//! [`TerminalView`]: one terminal on screen, wired to a [`TerminalHandle`].
//!
//! The view keeps the latest snapshot and forwards input. All terminal state lives in the
//! terminal's own task (see `ssherlock_terminal::handle`); this entity only holds what it needs
//! to paint the next frame and map mouse positions to cells.

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Global, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, WeakEntity, Window, canvas, div,
};
use ssherlock_terminal::{
    CellSide, GridPoint, Link, Modifiers, MouseAction, MouseInput, MouseReporting, PtyExit,
    ScrollAmount, SearchDirection, SearchStatus, SelectionKind, ShellConfig, Snapshot,
    TerminalConfig, TerminalError, TerminalEvent, TerminalHandle, TerminalSize,
    encode_alternate_scroll, encode_focus, encode_key, encode_mouse, encode_paste, is_multiline,
    spawn_local,
};

use super::actions::{
    ClearScrollback, Copy, DismissSearch, Find, Paste, ResetTerminal, ScrollLineDown, ScrollLineUp,
    ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop, SearchNewer, SearchOlder,
};
use super::element::{self, CellMetrics, PaintStyle};
use super::keys::{self, KeyInput};
use super::{DefaultShell, TerminalZoom};
use crate::components::{
    ContextMenu, InputKind, MenuDismissed, MenuItem, Modal, ModalEvent, TextInput, TextInputEvent,
};
use crate::platform;
use crate::runtime::TokioHandle;
use crate::theme::{ActiveTheme as _, Theme, from_rgb};

/// Something a [`TerminalView`] tells whoever shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalViewEvent {
    /// The program changed the title; read it with [`TerminalView::title`].
    TitleChanged,
    /// The program exited.
    Exited(PtyExit),
    /// The program rang the bell.
    Bell,
}

/// How long the visual bell shows.
const BELL_FLASH: Duration = Duration::from_millis(150);

/// Half a cursor blink: on for this long, then off for this long.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Where the view is in its life.
enum State {
    /// The shell is being started in the background.
    Starting,
    /// The terminal is running, or ran and exited (its last screen stays visible).
    Running {
        handle: TerminalHandle,
        snapshot: Arc<Snapshot>,
    },
    /// The shell couldn't be started; the message says why.
    Failed(SharedString),
}

/// A blinking cursor's state.
struct Blink {
    /// The cursor is drawn in this half of the cycle.
    is_on: bool,
    _task: Task<()>,
}

/// A multi-line paste waiting for the user to confirm it.
struct PendingPaste {
    text: String,
    modal: Entity<Modal>,
    _subscription: Subscription,
}

/// The open search bar.
struct SearchBar {
    input: Entity<TextInput>,
    _subscription: Subscription,
}

/// A terminal on screen.
pub struct TerminalView {
    state: State,
    focus_handle: FocusHandle,
    title: Option<SharedString>,
    exit: Option<PtyExit>,
    /// Measured on each render; used to turn mouse positions into cells.
    metrics: Option<CellMetrics>,
    /// Where the grid was painted last frame.
    grid_bounds: Option<Bounds<Pixels>>,
    /// The size last sent to the terminal, so a resize is sent only when it changes.
    requested_size: Option<TerminalSize>,
    /// The mouse button held down, for reporting drags and releases.
    pressed_button: Option<ssherlock_terminal::MouseButton>,
    /// A drag with the primary button is selecting text.
    is_selecting: bool,
    /// Wheel movement smaller than a line, carried over to the next event.
    scroll_remainder: f32,
    search: Option<SearchBar>,
    /// The link under the pointer while the link modifier is held.
    hovered_link: Option<Link>,
    /// Where the pointer last was over the grid, to look for a link when the modifier is
    /// pressed without moving.
    last_mouse: Option<Point<Pixels>>,
    /// The latest link lookup; replacing it cancels the previous one.
    link_lookup: Option<Task<()>>,
    pending_paste: Option<PendingPaste>,
    /// While the visual bell shows: the timer that turns it off.
    bell_flash: Option<Task<()>>,
    /// The terminal has keyboard focus.
    is_focused: bool,
    /// While the cursor blinks: its phase and the timer that flips it.
    blink: Option<Blink>,
    context_menu: Option<(Entity<ContextMenu>, Subscription)>,
    _subscriptions: Vec<Subscription>,
    /// Background work owned by the view; dropping the view cancels it.
    tasks: Vec<Task<()>>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    /// A view running a shell on this machine: the [`DefaultShell`] if one is set, else the
    /// user's login shell.
    pub fn local(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |view, _, cx| {
                view.focus_changed(true, cx);
            }),
            cx.on_blur(&focus_handle, window, |view, _, cx| {
                view.focus_changed(false, cx);
            }),
            cx.observe_global::<Theme>(Self::theme_changed),
            cx.observe_global::<TerminalZoom>(|_, cx| cx.notify()),
        ];
        let mut view = Self {
            state: State::Starting,
            focus_handle,
            title: None,
            exit: None,
            metrics: None,
            grid_bounds: None,
            requested_size: None,
            pressed_button: None,
            is_selecting: false,
            scroll_remainder: 0.,
            search: None,
            hovered_link: None,
            last_mouse: None,
            link_lookup: None,
            pending_paste: None,
            bell_flash: None,
            is_focused: false,
            blink: None,
            context_menu: None,
            _subscriptions: subscriptions,
            tasks: Vec::new(),
        };
        let shell = cx
            .try_global::<DefaultShell>()
            .map(|shell| shell.0.clone())
            .unwrap_or_default();
        view.start(shell, cx);
        view
    }

    /// The title the program set, if any.
    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    /// How the program exited, once it has.
    pub fn exit(&self) -> Option<&PtyExit> {
        self.exit.as_ref()
    }

    /// Starts `shell` off the foreground thread (spawning a process can take a while), then
    /// connects the terminal to it.
    fn start(&mut self, shell: ShellConfig, cx: &mut Context<Self>) {
        let size = self.requested_size.unwrap_or_default();
        let spawn = cx.background_spawn(async move { spawn_local(&shell, size) });
        let task = cx.spawn(async move |this, cx| {
            let result = spawn.await;
            let update = this.update(cx, |view, cx| match result {
                Ok(pty) => view.attach(pty, size, cx),
                Err(error) => {
                    let message = format!("{:#}", anyhow::Error::from(error));
                    tracing::error!(error = %message, "could not start a local shell");
                    view.state = State::Failed(message.into());
                    cx.notify();
                }
            });
            if update.is_err() {
                tracing::debug!("terminal view closed before its shell started");
            }
        });
        self.tasks.push(task);
    }

    /// Connects a started pty: runs the terminal on Tokio and listens for its snapshots and
    /// events.
    fn attach(
        &mut self,
        pty: ssherlock_terminal::PtyIo,
        size: TerminalSize,
        cx: &mut Context<Self>,
    ) {
        let Some(tokio) = cx.try_global::<TokioHandle>().map(|t| t.handle().clone()) else {
            self.state = State::Failed("the app's async runtime isn't running".into());
            cx.notify();
            return;
        };
        let config = TerminalConfig {
            palette: cx.theme().terminal_palette(),
            ..TerminalConfig::default()
        };
        let (handle, mut events) = TerminalHandle::spawn(pty, &config, size, &tokio);

        let mut snapshots = handle.subscribe();
        self.tasks.push(cx.spawn(async move |this, cx| {
            while snapshots.changed().await.is_ok() {
                let snapshot = Arc::clone(&snapshots.borrow_and_update());
                if this
                    .update(cx, |view, cx| view.set_snapshot(snapshot, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.tasks.push(cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.terminal_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));

        // The grid may have been laid out while the shell was starting.
        if let Some(requested) = self.requested_size.filter(|s| *s != size)
            && let Err(error) = handle.resize(requested)
        {
            tracing::warn!(%error, "could not resize the terminal");
        }
        self.state = State::Running {
            snapshot: handle.snapshot(),
            handle,
        };
        cx.notify();
    }

    fn set_snapshot(&mut self, new: Arc<Snapshot>, cx: &mut Context<Self>) {
        if let State::Running { snapshot, .. } = &mut self.state {
            *snapshot = new;
            self.update_blink(cx);
            cx.notify();
        }
    }

    /// Flashes the terminal briefly.
    fn ring_bell(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(BELL_FLASH);
        self.bell_flash = Some(cx.spawn(async move |this, cx| {
            timer.await;
            if this
                .update(cx, |view, cx| {
                    view.bell_flash = None;
                    cx.notify();
                })
                .is_err()
            {
                tracing::debug!("terminal view closed while its bell was showing");
            }
        }));
    }

    /// Starts or stops the blink loop: the cursor blinks while the terminal is focused and the
    /// program asked for a blinking cursor.
    fn update_blink(&mut self, cx: &mut Context<Self>) {
        let wants_blink = self.is_focused
            && self
                .handle()
                .and_then(|(_, snapshot)| snapshot.cursor)
                .is_some_and(|cursor| cursor.is_blinking);
        match (wants_blink, self.blink.is_some()) {
            (true, false) => self.restart_blink(cx),
            (false, true) => self.blink = None,
            _ => {}
        }
    }

    /// Shows the cursor and starts a fresh blink cycle, so it stays solid while you type.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                executor.timer(BLINK_INTERVAL).await;
                let toggled = this.update(cx, |view, cx| {
                    if let Some(blink) = &mut view.blink {
                        blink.is_on = !blink.is_on;
                        cx.notify();
                    }
                });
                if toggled.is_err() {
                    break;
                }
            }
        });
        self.blink = Some(Blink {
            is_on: true,
            _task: task,
        });
    }

    fn terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::TitleChanged(title) => {
                self.title = title.map(SharedString::from);
                cx.emit(TerminalViewEvent::TitleChanged);
            }
            TerminalEvent::Bell => {
                self.ring_bell(cx);
                cx.emit(TerminalViewEvent::Bell);
            }
            TerminalEvent::Exited(exit) => {
                self.exit = Some(exit.clone());
                cx.emit(TerminalViewEvent::Exited(exit));
            }
        }
        cx.notify();
    }

    fn handle(&self) -> Option<(&TerminalHandle, &Arc<Snapshot>)> {
        match &self.state {
            State::Running { handle, snapshot } => Some((handle, snapshot)),
            State::Starting | State::Failed(_) => None,
        }
    }

    fn write(&self, bytes: Vec<u8>) {
        let Some((handle, _)) = self.handle() else {
            return;
        };
        if let Err(error) = handle.write(bytes) {
            tracing::warn!(%error, "dropped input for the terminal");
        }
    }

    fn focus_changed(&mut self, is_focused: bool, cx: &mut Context<Self>) {
        self.is_focused = is_focused;
        self.update_blink(cx);
        if let Some((_, snapshot)) = self.handle()
            && let Some(bytes) = encode_focus(is_focused, snapshot.modes)
        {
            self.write(bytes.to_vec());
        }
        cx.notify();
    }

    fn theme_changed(&mut self, cx: &mut Context<Self>) {
        if let Some((handle, _)) = self.handle()
            && let Err(error) = handle.set_palette(cx.theme().terminal_palette())
        {
            tracing::warn!(%error, "could not update the terminal's colours");
        }
    }

    /// Called while painting, with the space the grid has: resizes the terminal to fit.
    fn layout_grid(&mut self, bounds: Bounds<Pixels>) {
        self.grid_bounds = Some(bounds);
        let Some(metrics) = &self.metrics else {
            return;
        };
        let size = metrics.grid_size(bounds.size);
        if self.requested_size == Some(size) {
            return;
        }
        self.requested_size = Some(size);
        if let Some((handle, _)) = self.handle()
            && let Err(error) = handle.resize(size)
        {
            tracing::warn!(%error, "could not resize the terminal");
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((_, snapshot)) = self.handle() else {
            return;
        };
        let bytes = match keys::key_input(&event.keystroke, platform::ALT_KEY) {
            Some(KeyInput::Text(text)) => text.into_bytes(),
            Some(KeyInput::Key(key, modifiers)) => {
                let Some(bytes) = encode_key(key, modifiers, snapshot.modes) else {
                    return;
                };
                bytes
            }
            None => return,
        };
        self.write(bytes);
        if self.blink.is_some() {
            self.restart_blink(cx);
        }
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(Clipboard::Main, cx);
    }

    /// Reads the selection from the terminal and puts it on `clipboard`.
    fn copy_selection(&self, clipboard: Clipboard, cx: &mut Context<Self>) {
        let Some((handle, _)) = self.handle() else {
            tracing::debug!(?clipboard, "not copying: the terminal isn't running");
            return;
        };
        let request = match handle.selection_text() {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(%error, "could not read the selection");
                return;
            }
        };
        // Resolves as soon as the terminal's task answers, or with `None` if it stops, so the
        // task can't outlive the view by more than that.
        cx.spawn(async move |_, cx| {
            let Some(text) = request.await else {
                tracing::debug!(?clipboard, "nothing selected to copy");
                return;
            };
            // The length only: the text itself may be a secret printed in the terminal.
            tracing::debug!(
                ?clipboard,
                chars = text.chars().count(),
                "copying the selection"
            );
            let item = ClipboardItem::new_string(text);
            let written = cx.update(|cx| match clipboard {
                Clipboard::Main => cx.write_to_clipboard(item),
                Clipboard::Primary => cx.write_to_primary(item),
            });
            if written.is_err() {
                tracing::debug!("app closed before the selection was copied");
            }
        })
        .detach();
    }

    /// Pastes `text`, first asking for confirmation if it has several lines and the program
    /// hasn't turned on bracketed paste: then every line break presses Enter, so the lines run
    /// as commands as soon as they arrive.
    fn paste_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, snapshot)) = self.handle() else {
            return;
        };
        if snapshot.modes.is_bracketed_paste() || !is_multiline(&text) {
            let bytes = encode_paste(&text, snapshot.modes);
            self.write(bytes);
            return;
        }

        let (preview, more) = paste_preview(&text, PASTE_PREVIEW_LINES);
        let lines = preview.len() + more;
        let family = cx
            .try_global::<MonoFamily>()
            .map(|MonoFamily(family)| family.clone());
        let modal = cx.new(|cx| {
            Modal::new(
                "Paste several lines?",
                format!(
                    "This pastes {lines} lines. Each line break runs what comes before it as a \
                     command."
                ),
                "Paste",
                window,
                cx,
            )
            .detail(move |_, cx| paste_detail(&preview, more, family.clone(), cx))
        });
        let subscription = cx.subscribe_in(&modal, window, |view, _, event, window, cx| {
            let pending = view.pending_paste.take();
            if let (ModalEvent::Confirmed, Some(pending)) = (event, pending)
                && let Some((_, snapshot)) = view.handle()
            {
                let bytes = encode_paste(&pending.text, snapshot.modes);
                view.write(bytes);
            }
            view.focus_handle.focus(window);
            cx.notify();
        });
        self.pending_paste = Some(PendingPaste {
            text,
            modal,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Opens the right-click menu at `position`.
    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_selection = self
            .handle()
            .is_some_and(|(_, snapshot)| snapshot.selection.is_some());
        let items = vec![
            MenuItem::new("Copy", Copy).enabled(has_selection),
            MenuItem::new("Paste", Paste),
            MenuItem::new("Find", Find),
            MenuItem::new("Clear Scrollback", ClearScrollback),
            MenuItem::new("Reset Terminal", ResetTerminal),
        ];
        let target = self.focus_handle.clone();
        let menu = cx.new(|cx| ContextMenu::new(items, position, target, window, cx));
        let subscription = cx.subscribe(&menu, |view, _, _: &MenuDismissed, cx| {
            view.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((menu, subscription));
        cx.notify();
    }

    /// The cell under `position`, if the grid has been laid out.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let (_, snapshot) = self.handle()?;
        let metrics = self.metrics.as_ref()?;
        let bounds = self.grid_bounds?;
        Some(metrics.cell_at(position, bounds.origin, snapshot.rows(), snapshot.columns()))
    }

    /// The cell and side under `position`, for selecting.
    fn selection_point(&self, position: Point<Pixels>) -> Option<(GridPoint, CellSide)> {
        let (_, snapshot) = self.handle()?;
        let metrics = self.metrics.as_ref()?;
        let bounds = self.grid_bounds?;
        let (row, column) = self.cell_at(position)?;
        let side = metrics.side_at(position, bounds.origin, snapshot.columns());
        Some((GridPoint::new(row, column), side))
    }

    /// Reports a mouse event to the program if it asked for mouse input. Shift bypasses
    /// reporting, as in xterm, so text stays selectable in mouse-aware programs.
    fn report_mouse(
        &self,
        action: MouseAction,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let Some((_, snapshot)) = self.handle() else {
            return false;
        };
        if snapshot.modes.mouse_reporting() == MouseReporting::Off || modifiers.shift {
            return false;
        }
        let Some((row, column)) = self.cell_at(position) else {
            return false;
        };
        let input = MouseInput {
            action,
            row,
            column,
            modifiers: terminal_modifiers(modifiers),
        };
        if let Some(bytes) = encode_mouse(input, snapshot.modes) {
            self.write(bytes);
        }
        true
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if event.button == MouseButton::Left
            && platform::is_link_modifier(&event.modifiers)
            && let Some(link) = &self.hovered_link
        {
            if link.is_safe_to_open() {
                cx.open_url(&link.uri);
            } else {
                tracing::info!(uri = %link.uri, "not opening a link with an unsupported scheme");
            }
            cx.stop_propagation();
            return;
        }
        let Some(button) = terminal_button(event.button) else {
            return;
        };
        self.pressed_button = Some(button);
        if self.report_mouse(MouseAction::Press(button), event.position, event.modifiers) {
            cx.stop_propagation();
            return;
        }
        match event.button {
            MouseButton::Left => self.start_selection(event, cx),
            MouseButton::Middle if platform::HAS_PRIMARY_SELECTION => {
                if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
                    self.paste_text(text, window, cx);
                }
                cx.stop_propagation();
            }
            MouseButton::Right => {
                self.open_context_menu(event.position, window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Starts selecting at the pointer: a drag selects characters, a double click words, a
    /// triple click lines, and Alt/Option-drag a rectangle.
    fn start_selection(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some((handle, _)) = self.handle() else {
            return;
        };
        let Some((point, side)) = self.selection_point(event.position) else {
            return;
        };
        let kind = match event.click_count {
            2 => SelectionKind::Word,
            n if n >= 3 => SelectionKind::Line,
            _ if event.modifiers.alt => SelectionKind::Block,
            _ => SelectionKind::Simple,
        };
        if let Err(error) = handle.start_selection(kind, point, side) {
            tracing::warn!(%error, "could not start a selection");
            return;
        }
        self.is_selecting = true;
        cx.stop_propagation();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(button) = terminal_button(event.button) else {
            return;
        };
        self.pressed_button = None;
        if self.is_selecting && event.button == MouseButton::Left {
            self.is_selecting = false;
            // On X11 and Wayland, selecting is copying to the primary selection.
            if platform::HAS_PRIMARY_SELECTION {
                self.copy_selection(Clipboard::Primary, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.report_mouse(
            MouseAction::Release(button),
            event.position,
            event.modifiers,
        ) {
            cx.stop_propagation();
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_mouse = Some(event.position);
        self.update_hovered_link(event.modifiers, cx);
        if self.is_selecting && event.pressed_button == Some(MouseButton::Left) {
            if let Some((handle, _)) = self.handle()
                && let Some((point, side)) = self.selection_point(event.position)
                && let Err(error) = handle.extend_selection(point, side)
            {
                tracing::warn!(%error, "could not extend the selection");
            }
            cx.stop_propagation();
            return;
        }
        let button = event.pressed_button.and(self.pressed_button);
        self.report_mouse(MouseAction::Move(button), event.position, event.modifiers);
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let lines = match event.delta {
            // Already in lines: the platform applies its own lines-per-notch (3 on X11).
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / f32::from(metrics.line_height),
        } + self.scroll_remainder;
        let whole = lines.trunc();
        self.scroll_remainder = lines - whole;
        // Wheel steps are small numbers; anything outside i32 is nonsense from the platform.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "clamped to a few hundred lines"
        )]
        let steps = whole.clamp(-500., 500.) as i32;
        if steps == 0 {
            return;
        }

        let Some((_, snapshot)) = self.handle() else {
            return;
        };
        let modes = snapshot.modes;
        let button = if steps > 0 {
            ssherlock_terminal::MouseButton::WheelUp
        } else {
            ssherlock_terminal::MouseButton::WheelDown
        };
        if modes.mouse_reporting() != MouseReporting::Off && !event.modifiers.shift {
            for _ in 0..steps.unsigned_abs() {
                self.report_mouse(MouseAction::Press(button), event.position, event.modifiers);
            }
        } else if let Some(bytes) = encode_alternate_scroll(steps, modes) {
            self.write(bytes);
        } else {
            self.run(TerminalHandle::scroll, ScrollAmount::Lines(steps));
        }
        cx.stop_propagation();
    }

    fn modifiers_changed(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_hovered_link(event.modifiers, cx);
    }

    /// Looks for a link under the pointer while the link modifier is held, and forgets it
    /// otherwise.
    fn update_hovered_link(&mut self, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let position = self
            .last_mouse
            .filter(|_| platform::is_link_modifier(&modifiers) && !self.is_selecting);
        let request = position
            .and_then(|position| self.cell_at(position))
            .and_then(|(row, column)| {
                let (handle, _) = self.handle()?;
                handle.link_at(GridPoint::new(row, column)).ok()
            });
        let Some(request) = request else {
            self.link_lookup = None;
            if self.hovered_link.take().is_some() {
                cx.notify();
            }
            return;
        };
        self.link_lookup = Some(cx.spawn(async move |this, cx| {
            let link = request.await;
            let updated = this.update(cx, |view, cx| {
                if view.hovered_link != link {
                    view.hovered_link = link;
                    cx.notify();
                }
            });
            if updated.is_err() {
                tracing::debug!("terminal view closed during a link lookup");
            }
        }));
    }

    /// Calls `command` on the running terminal with `argument`, logging a failure.
    fn run<A>(&self, command: fn(&TerminalHandle, A) -> Result<(), TerminalError>, argument: A) {
        if let Some((handle, _)) = self.handle()
            && let Err(error) = command(handle, argument)
        {
            tracing::warn!(%error, "terminal command dropped");
        }
    }

    fn scroll_line_up(&mut self, _: &ScrollLineUp, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::Lines(1));
    }

    fn scroll_line_down(&mut self, _: &ScrollLineDown, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::Lines(-1));
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::PageUp);
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::PageDown);
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::Top);
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::scroll, ScrollAmount::Bottom);
    }

    fn clear_scrollback(&mut self, _: &ClearScrollback, _: &mut Window, _: &mut Context<Self>) {
        self.run(|handle, ()| handle.clear_scrollback(), ());
    }

    fn reset_terminal(&mut self, _: &ResetTerminal, _: &mut Window, _: &mut Context<Self>) {
        self.run(|handle, ()| handle.reset(), ());
    }

    /// Opens the search bar, or focuses it and selects its text if it is already open.
    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search {
            search.input.update(cx, TextInput::select_all);
            search.input.focus_handle(cx).focus(window);
            return;
        }
        let input = cx.new(|cx| TextInput::new("", InputKind::Persistent, window, cx));
        let subscription = cx.subscribe(&input, |view, _, event: &TextInputEvent, _| {
            if let TextInputEvent::Changed(query) = event {
                view.run(TerminalHandle::set_search, query.clone());
            }
        });
        self.search = Some(SearchBar {
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn search_older(&mut self, _: &SearchOlder, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::search_next, SearchDirection::Older);
    }

    fn search_newer(&mut self, _: &SearchNewer, _: &mut Window, _: &mut Context<Self>) {
        self.run(TerminalHandle::search_next, SearchDirection::Newer);
    }

    fn dismiss_search(&mut self, _: &DismissSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.run(|handle, ()| handle.clear_search(), ());
        self.focus_handle.focus(window);
        cx.notify();
    }
}

/// Where copied text goes.
#[derive(Clone, Copy, Debug)]
enum Clipboard {
    /// The clipboard that Copy and Paste use.
    Main,
    /// The X11/Wayland primary selection, filled by selecting and pasted with the middle button.
    Primary,
}

fn terminal_modifiers(modifiers: gpui::Modifiers) -> Modifiers {
    Modifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

fn terminal_button(button: MouseButton) -> Option<ssherlock_terminal::MouseButton> {
    match button {
        MouseButton::Left => Some(ssherlock_terminal::MouseButton::Left),
        MouseButton::Middle => Some(ssherlock_terminal::MouseButton::Middle),
        MouseButton::Right => Some(ssherlock_terminal::MouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

/// The monospace family terminals use: the first of the theme's candidates that is installed,
/// looked up once per run.
fn mono_family(theme: &Theme, window: &Window, cx: &mut App) -> SharedString {
    if let Some(MonoFamily(family)) = cx.try_global::<MonoFamily>() {
        return family.clone();
    }
    let installed = window.text_system().all_font_names();
    let candidates = theme.text.mono_families;
    let family = candidates
        .iter()
        .find(|candidate| installed.iter().any(|name| name == *candidate))
        .or(candidates.first())
        .map_or_else(
            || SharedString::new_static(".ZedMono"),
            |f| SharedString::new_static(f),
        );
    tracing::info!(%family, "terminal font");
    cx.set_global(MonoFamily(family.clone()));
    family
}

/// The resolved terminal font family, cached for the life of the app.
struct MonoFamily(SharedString);

impl Global for MonoFamily {}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let family = mono_family(&theme, window, cx);
        // Focus events don't arrive for focus given before the first frame (a new window or
        // tab), so the frame itself keeps the blink in step with focus.
        let is_focused = self.focus_handle.is_focused(window);
        if is_focused != self.is_focused {
            self.is_focused = is_focused;
            self.update_blink(cx);
        }
        let zoom = cx
            .try_global::<TerminalZoom>()
            .copied()
            .unwrap_or_default()
            .0;
        let metrics = CellMetrics::measure(&theme, family, zoom, window);
        self.metrics = Some(metrics.clone());

        let background = match &self.state {
            State::Running { snapshot, .. } => from_rgb(snapshot.background),
            State::Starting | State::Failed(_) => theme.terminal.background,
        };
        // The grid takes keys and the mouse. The search bar sits beside it, not inside it, so
        // typing in the search box never reaches the program. Actions from either bubble up to
        // `root`.
        let area = div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .cursor(if self.hovered_link.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            .flex()
            .flex_col()
            .size_full()
            .p(theme.spacing.md)
            .bg(background);
        let area = match &self.state {
            State::Starting => area,
            State::Failed(message) => area.child(
                div()
                    .text_color(theme.terminal.foreground)
                    .text_size(theme.text.ui_size)
                    .child(format!("Could not start a shell: {message}")),
            ),
            State::Running { snapshot, .. } => {
                let style = PaintStyle {
                    metrics,
                    is_focused,
                    is_cursor_visible: self.blink.as_ref().is_none_or(|blink| blink.is_on),
                    bell_flash: self.bell_flash.as_ref().map(|_| theme.terminal.bell_flash),
                    cursor_thickness: theme.metrics.cursor_thickness,
                    selection: theme.terminal.selection,
                    search_match: theme.terminal.search_match,
                    search_match_focused: theme.terminal.search_match_focused,
                    scrollbar: theme.colors.scrollbar,
                    scrollbar_width: theme.metrics.scrollbar_width,
                    link: self.hovered_link.as_ref().map(|link| link.range),
                    link_color: theme.colors.accent,
                    link_thickness: theme.metrics.border_width,
                };
                area.child(grid(cx.weak_entity(), Arc::clone(snapshot), style))
            }
        };
        let status = match &self.state {
            State::Running { snapshot, .. } => snapshot.search_status,
            State::Starting | State::Failed(_) => None,
        };

        div()
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::scroll_line_up))
            .on_action(cx.listener(Self::scroll_line_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(Self::clear_scrollback))
            .on_action(cx.listener(Self::reset_terminal))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::search_older))
            .on_action(cx.listener(Self::search_newer))
            .on_action(cx.listener(Self::dismiss_search))
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .child(area)
            .when_some(self.search.as_ref(), |root, search| {
                root.child(search_bar(&search.input, status, &theme, cx))
            })
            .when_some(self.hovered_link.as_ref(), |root, link| {
                root.child(link_hint(link, &theme))
            })
            .when_some(self.exit.as_ref(), |root, exit| {
                root.child(exit_banner(exit, &theme))
            })
            .when_some(self.pending_paste.as_ref(), |root, pending| {
                root.child(pending.modal.clone())
            })
            .when_some(self.context_menu.as_ref(), |root, (menu, _)| {
                root.child(menu.clone())
            })
    }
}

/// Lines of a multi-line paste shown in its confirmation.
const PASTE_PREVIEW_LINES: usize = 6;

/// Longest preview line, in characters, before it is cut with an ellipsis.
const PASTE_PREVIEW_WIDTH: usize = 80;

/// The first `max` lines of `text` for a preview (each cut to [`PASTE_PREVIEW_WIDTH`]), and how
/// many lines are left out. A final line break doesn't count as a line.
fn paste_preview(text: &str, max: usize) -> (Vec<String>, usize) {
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let shown = lines
        .iter()
        .take(max)
        .map(|line| {
            if line.chars().count() > PASTE_PREVIEW_WIDTH {
                let cut: String = line.chars().take(PASTE_PREVIEW_WIDTH).collect();
                format!("{cut}\u{2026}")
            } else {
                (*line).to_owned()
            }
        })
        .collect();
    (shown, lines.len().saturating_sub(max))
}

/// The preview box inside the paste confirmation.
fn paste_detail(
    lines: &[String],
    more: usize,
    family: Option<SharedString>,
    cx: &App,
) -> gpui::AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .p(theme.spacing.md)
        .rounded(theme.radius.sm)
        .bg(theme.terminal.background)
        .text_color(theme.terminal.foreground)
        .text_size(theme.text.small_size)
        .when_some(family, gpui::Styled::font_family)
        .overflow_hidden()
        .children(lines.iter().map(|line| {
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(SharedString::from(line.clone()))
        }))
        .when(more > 0, |d| {
            d.child(
                div()
                    .text_color(theme.colors.text_muted)
                    .child(format!("\u{2026} and {more} more")),
            )
        })
        .into_any_element()
}

/// Where the link under the pointer goes, shown before it is opened: an `OSC 8` link's text
/// can say anything, so the real target is what the user decides on.
fn link_hint(link: &Link, theme: &Theme) -> impl IntoElement {
    let text = if link.is_safe_to_open() {
        format!(
            "{}+click to open {}",
            platform::LINK_MODIFIER_NAME,
            link.uri
        )
    } else {
        format!("{} (not opened: unsupported link type)", link.uri)
    };
    div()
        .absolute()
        .bottom(theme.spacing.md)
        .left(theme.spacing.md)
        .max_w(theme.metrics.tab_max_width * 3.)
        .px(theme.spacing.md)
        .py(theme.spacing.xs)
        .rounded(theme.radius.sm)
        .bg(theme.colors.surface)
        .border(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .text_size(theme.text.small_size)
        .text_color(theme.colors.text_muted)
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(text)
}

/// The search box floating over the top-right corner of the terminal.
fn search_bar(
    input: &Entity<TextInput>,
    status: Option<SearchStatus>,
    theme: &Theme,
    cx: &mut Context<TerminalView>,
) -> impl IntoElement {
    let has_query = !input.read(cx).text().is_empty();
    let count = match status {
        _ if !has_query => String::new(),
        None => String::new(),
        Some(SearchStatus { total: 0, .. }) => "No matches".to_owned(),
        Some(SearchStatus {
            total,
            current,
            is_capped,
        }) => {
            let more = if is_capped { "+" } else { "" };
            match current {
                Some(current) => format!("{current} of {total}{more}"),
                None => format!("{total}{more} matches"),
            }
        }
    };
    let button = |id: &'static str, label: &'static str| {
        let hover = theme.colors.control_hover;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(theme.metrics.window_control_size)
            .rounded(theme.radius.sm)
            .text_color(theme.colors.text_muted)
            .hover(move |style| style.bg(hover))
            .child(label)
    };

    div()
        .key_context("TerminalSearch")
        .absolute()
        .top(theme.spacing.md)
        .right(theme.spacing.xl)
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .px(theme.spacing.md)
        .py(theme.spacing.xs)
        .rounded(theme.radius.md)
        .bg(theme.colors.surface)
        .border(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .text_size(theme.text.small_size)
        .child(
            div()
                .w(theme.metrics.search_field_width)
                .child(input.clone()),
        )
        .child(div().text_color(theme.colors.text_muted).child(count))
        .child(
            button("search-older", "\u{2191}").on_click(cx.listener(|view, _, window, cx| {
                view.search_older(&SearchOlder, window, cx);
            })),
        )
        .child(
            button("search-newer", "\u{2193}").on_click(cx.listener(|view, _, window, cx| {
                view.search_newer(&SearchNewer, window, cx);
            })),
        )
        .child(
            button("search-close", "\u{d7}").on_click(cx.listener(|view, _, window, cx| {
                view.dismiss_search(&DismissSearch, window, cx);
            })),
        )
}

/// The painted grid. Reports its bounds back to the view so the terminal can be resized to fit
/// and mouse positions mapped to cells.
fn grid(
    view: WeakEntity<TerminalView>,
    snapshot: Arc<Snapshot>,
    style: PaintStyle,
) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            if view.update(cx, |view, _| view.layout_grid(bounds)).is_err() {
                // The view is gone; nothing will read the bounds.
                tracing::debug!("terminal view dropped during layout");
            }
        },
        move |bounds, (), window, cx| element::paint(bounds, &snapshot, &style, window, cx),
    )
    .size_full()
}

fn exit_banner(exit: &PtyExit, theme: &Theme) -> impl IntoElement {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .px(theme.spacing.lg)
        .py(theme.spacing.sm)
        .bg(theme.colors.surface)
        .border_t(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .text_color(theme.colors.text_muted)
        .text_size(theme.text.small_size)
        .child(format!("Process {exit}"))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn paste_preview_shows_the_first_lines() {
        let cases = [
            ("two lines", "ls\npwd", 6, (vec!["ls", "pwd"], 0)),
            (
                "trailing newline ignored",
                "ls\npwd\n",
                6,
                (vec!["ls", "pwd"], 0),
            ),
            ("crlf", "ls\r\npwd\r\n", 6, (vec!["ls", "pwd"], 0)),
            ("more than shown", "1\n2\n3\n4", 2, (vec!["1", "2"], 2)),
        ];
        for (name, text, max, (lines, more)) in cases {
            let expected: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
            assert_eq!(paste_preview(text, max), (expected, more), "{name}");
        }

        let long = "x".repeat(PASTE_PREVIEW_WIDTH + 5);
        let cut = format!("{}\u{2026}", "x".repeat(PASTE_PREVIEW_WIDTH));
        let (shown, _) = paste_preview(&format!("{long}\nb"), 6);
        assert_eq!(shown, vec![cut, "b".to_owned()]);
    }
}
