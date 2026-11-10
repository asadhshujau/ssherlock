//! [`TerminalView`]: one terminal on screen, wired to a [`TerminalHandle`].
//!
//! The view keeps the latest snapshot and forwards input. All terminal state lives in the
//! terminal's own task (see `ssherlock_terminal::handle`); this entity only holds what it needs
//! to paint the next frame and map mouse positions to cells.

use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, Context, EventEmitter, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled as _, Subscription, Task, WeakEntity, Window, canvas,
    div,
};
use ssherlock_terminal::{
    Modifiers, MouseAction, MouseInput, MouseReporting, PtyExit, ShellConfig, Snapshot,
    TerminalConfig, TerminalEvent, TerminalHandle, TerminalSize, encode_alternate_scroll,
    encode_focus, encode_key, encode_mouse, encode_paste, spawn_local,
};

use super::DefaultShell;
use super::actions::Paste;
use super::element::{self, CellMetrics, PaintStyle};
use super::keys::{self, KeyInput};
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
}

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
    /// Wheel movement smaller than a line, carried over to the next event.
    scroll_remainder: f32,
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
            scroll_remainder: 0.,
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
            cx.notify();
        }
    }

    fn terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::TitleChanged(title) => {
                self.title = title.map(SharedString::from);
                cx.emit(TerminalViewEvent::TitleChanged);
            }
            // The visual and audible bell arrive with TERM-13.
            TerminalEvent::Bell => {}
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
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some((_, snapshot)) = self.handle() else {
            return;
        };
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bytes = encode_paste(&text, snapshot.modes);
        self.write(bytes);
    }

    /// The cell under `position` and the snapshot it belongs to, if the grid has been laid out.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let (_, snapshot) = self.handle()?;
        let metrics = self.metrics.as_ref()?;
        let bounds = self.grid_bounds?;
        Some(metrics.cell_at(position, bounds.origin, snapshot.rows(), snapshot.columns()))
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
        let Some(button) = terminal_button(event.button) else {
            return;
        };
        self.pressed_button = Some(button);
        if self.report_mouse(MouseAction::Press(button), event.position, event.modifiers) {
            cx.stop_propagation();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(button) = terminal_button(event.button) else {
            return;
        };
        self.pressed_button = None;
        if self.report_mouse(
            MouseAction::Release(button),
            event.position,
            event.modifiers,
        ) {
            cx.stop_propagation();
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        let button = event.pressed_button.and(self.pressed_button);
        self.report_mouse(MouseAction::Move(button), event.position, event.modifiers);
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let lines = match event.delta {
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
            // Scrolling the scrollback arrives with TERM-08.
            return;
        }
        cx.stop_propagation();
    }
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
        let metrics = CellMetrics::measure(&theme, family, window);
        self.metrics = Some(metrics.clone());

        let background = match &self.state {
            State::Running { snapshot, .. } => from_rgb(snapshot.background),
            State::Starting | State::Failed(_) => theme.terminal.background,
        };
        let root = div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::paste))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .p(theme.spacing.md)
            .bg(background);

        match &self.state {
            State::Starting => root,
            State::Failed(message) => root.child(
                div()
                    .text_color(theme.terminal.foreground)
                    .text_size(theme.text.ui_size)
                    .child(format!("Could not start a shell: {message}")),
            ),
            State::Running { snapshot, .. } => {
                let style = PaintStyle {
                    metrics,
                    is_focused: self.focus_handle.is_focused(window),
                    cursor_thickness: theme.metrics.cursor_thickness,
                };
                let root = root.child(grid(cx.weak_entity(), Arc::clone(snapshot), style));
                match &self.exit {
                    None => root,
                    Some(exit) => root.child(exit_banner(exit, &theme)),
                }
            }
        }
    }
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
