//! [`Pane`]: a tab bar over the active tab's terminal.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EntityId, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::ConnectTarget;

use super::actions::{
    CloseTab, MoveTabLeft, MoveTabRight, NewTab, NextTab, PreviousTab, RenameTab,
};
use super::list::{Step, TabList};
use crate::components::{InputKind, Tab, TabBar, TextInput, TextInputEvent};
use crate::ssh::actions::QuickConnect;
use crate::ssh::{QuickConnectBar, QuickConnectEvent, RemoteTerminal};
use crate::terminal::{TerminalView, TerminalViewEvent};
use crate::theme::ActiveTheme as _;

/// Shown for a tab whose program hasn't set a title.
const DEFAULT_TITLE: &str = "Terminal";

/// What a tab shows: a shell on this machine or on a server.
enum TabView {
    Local(Entity<TerminalView>),
    Remote(Entity<RemoteTerminal>),
}

impl TabView {
    fn entity_id(&self) -> EntityId {
        match self {
            Self::Local(view) => view.entity_id(),
            Self::Remote(view) => view.entity_id(),
        }
    }

    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        match self {
            Self::Local(view) => view.focus_handle(cx),
            Self::Remote(view) => view.focus_handle(cx),
        }
    }

    /// The program's title, else (for a remote tab) `user@host`.
    fn title(&self, cx: &gpui::App) -> Option<SharedString> {
        match self {
            Self::Local(view) => view.read(cx).title().cloned(),
            Self::Remote(view) => {
                let view = view.read(cx);
                view.title(cx).or_else(|| Some(view.label().clone()))
            }
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Self::Local(view) => view.clone().into_any_element(),
            Self::Remote(view) => view.clone().into_any_element(),
        }
    }
}

/// One tab: a terminal and the name the user gave it, if any.
struct PaneTab {
    view: TabView,
    /// Chosen by the user; wins over the program's title.
    name: Option<SharedString>,
    /// The bell rang while the tab was in the background.
    has_bell: bool,
    _subscription: Subscription,
}

/// An in-progress rename.
struct Rename {
    tab: EntityId,
    input: Entity<TextInput>,
    _subscription: Subscription,
}

/// A tab being dragged to a new place.
#[derive(Clone, Debug)]
struct DraggedTab {
    tab: EntityId,
    label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(theme.spacing.lg)
            .py(theme.spacing.sm)
            .rounded(theme.radius.sm)
            .bg(theme.colors.tab_active)
            .border(theme.metrics.border_width)
            .border_color(theme.colors.accent)
            .text_size(theme.text.small_size)
            .text_color(theme.colors.text)
            .child(self.label.clone())
    }
}

/// A tab bar over the active tab's terminal.
pub struct Pane {
    tabs: TabList<PaneTab>,
    rename: Option<Rename>,
    quick_connect: Option<(Entity<QuickConnectBar>, Subscription)>,
    focus_handle: FocusHandle,
}

impl Focusable for Pane {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if let Some((bar, _)) = &self.quick_connect {
            return bar.focus_handle(cx);
        }
        self.tabs.active().map_or_else(
            || self.focus_handle.clone(),
            |tab| tab.view.focus_handle(cx),
        )
    }
}

impl Pane {
    /// A pane with one local terminal tab.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            tabs: TabList::default(),
            rename: None,
            quick_connect: None,
            focus_handle: cx.focus_handle(),
        };
        pane.open_tab(window, cx);
        pane
    }

    /// The active tab's label, for the window title.
    pub fn title(&self, cx: &gpui::App) -> SharedString {
        self.tabs.active().map_or_else(
            || SharedString::new_static(DEFAULT_TITLE),
            |tab| label(tab, cx),
        )
    }

    /// Number of open tabs.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Index of the active tab.
    pub fn active_index(&self) -> Option<usize> {
        self.tabs.active_index()
    }

    fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| TerminalView::local(window, cx));
        let subscription = cx.subscribe_in(&view, window, |pane, view, event, window, cx| {
            pane.terminal_event(view.entity_id(), event, window, cx);
        });
        self.add_tab(TabView::Local(view), subscription, window, cx);
    }

    /// Opens a tab connected to `target` over SSH, after the active one.
    pub fn open_remote_tab(
        &mut self,
        target: &ConnectTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| RemoteTerminal::new(target, window, cx));
        let subscription = cx.subscribe_in(&view, window, |pane, view, event, window, cx| {
            pane.terminal_event(view.entity_id(), event, window, cx);
        });
        self.add_tab(TabView::Remote(view), subscription, window, cx);
    }

    fn add_tab(
        &mut self,
        view: TabView,
        subscription: Subscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tabs.open(PaneTab {
            view,
            name: None,
            has_bell: false,
            _subscription: subscription,
        });
        self.focus_active(window, cx);
    }

    fn quick_connect(&mut self, _: &QuickConnect, window: &mut Window, cx: &mut Context<Self>) {
        let bar = cx.new(|cx| QuickConnectBar::new(window, cx));
        let subscription = cx.subscribe_in(&bar, window, |pane, _, event, window, cx| {
            pane.quick_connect = None;
            match event {
                QuickConnectEvent::Connect(target) => pane.open_remote_tab(target, window, cx),
                QuickConnectEvent::Dismissed => pane.focus_active(window, cx),
            }
        });
        bar.focus_handle(cx).focus(window);
        self.quick_connect = Some((bar, subscription));
        cx.notify();
    }

    /// Whether the quick connect bar is open.
    pub fn is_quick_connect_open(&self) -> bool {
        self.quick_connect.is_some()
    }

    fn terminal_event(
        &mut self,
        id: EntityId,
        event: &TerminalViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *event == TerminalViewEvent::Bell {
            let active = self.tabs.active().map(|tab| tab.view.entity_id());
            if active != Some(id)
                && let Some(index) = self.tabs.position(|tab| tab.view.entity_id() == id)
                && let Some(tab) = self.tabs.get_mut(index)
            {
                tab.has_bell = true;
                cx.notify();
            }
            return;
        }
        // A shell that ended normally (`exit`, Ctrl+D) closes its tab. A new title, or a shell
        // that failed, only needs a redraw: the failed one stays open so its last output and
        // status can be read.
        if !is_clean_exit(event) {
            cx.notify();
            return;
        }
        if let Some(index) = self.tabs.position(|tab| tab.view.entity_id() == id) {
            self.close_tab_at(index, window, cx);
        }
    }

    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.close(index) else {
            return;
        };
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.tab == tab.view.entity_id())
        {
            self.rename = None;
        }
        // Dropping the tab drops its view, which stops the terminal and hangs up the shell.
        drop(tab);
        self.focus_active(window, cx);
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.active_index()
            && let Some(tab) = self.tabs.get_mut(index)
        {
            tab.has_bell = false;
        }
        match self.tabs.active() {
            Some(tab) => tab.view.focus_handle(cx).focus(window),
            None => self.focus_handle.focus(window),
        }
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.active_index() {
            self.close_tab_at(index, window, cx);
        }
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate_next();
        self.focus_active(window, cx);
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate_previous();
        self.focus_active(window, cx);
    }

    fn move_tab_left(&mut self, _: &MoveTabLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.tabs.move_active(Step::Left);
        cx.notify();
    }

    fn move_tab_right(&mut self, _: &MoveTabRight, _: &mut Window, cx: &mut Context<Self>) {
        self.tabs.move_active(Step::Right);
        cx.notify();
    }

    fn rename_tab(&mut self, _: &RenameTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.active_index() {
            self.start_rename(index, window, cx);
        }
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate(index);
        self.focus_active(window, cx);
    }

    /// Replaces the label of the tab at `index` with a text field.
    fn start_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let id = tab.view.entity_id();
        let current = label(tab, cx);
        let input = cx.new(|cx| TextInput::new(current, InputKind::SingleEdit, window, cx));
        let subscription = cx.subscribe_in(&input, window, move |pane, _, event, window, cx| {
            pane.finish_rename(id, event, window, cx);
        });
        self.rename = Some(Rename {
            tab: id,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn finish_rename(
        &mut self,
        id: EntityId,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TextInputEvent::Changed(_) = event {
            return;
        }
        if let TextInputEvent::Confirmed(text) = event
            && let Some(index) = self.tabs.position(|tab| tab.view.entity_id() == id)
            && let Some(tab) = self.tabs.get_mut(index)
        {
            // Clearing the name goes back to the program's title.
            let name = text.trim();
            tab.name = (!name.is_empty()).then(|| SharedString::from(name.to_owned()));
        }
        self.rename = None;
        self.focus_active(window, cx);
    }

    /// Moves the dragged tab to `to`.
    fn drop_tab(&mut self, dragged: &DraggedTab, to: usize, cx: &mut Context<Self>) {
        if let Some(from) = self
            .tabs
            .position(|tab| tab.view.entity_id() == dragged.tab)
        {
            self.tabs.move_tab(from, to);
            cx.notify();
        }
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> TabBar {
        let last = self.tabs.len().saturating_sub(1);
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let id = tab.view.entity_id();
            let text = label(tab, cx);
            let label = match &self.rename {
                Some(rename) if rename.tab == id => rename.input.clone().into_any_element(),
                _ => text.clone().into_any_element(),
            };
            let dragged = DraggedTab {
                tab: id,
                label: text,
            };
            Tab::new(("tab", id.as_u64()), label)
                .active(self.tabs.active_index() == Some(index))
                .indicator(tab.has_bell)
                .on_click(
                    cx.listener(move |pane, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() == 2 {
                            pane.start_rename(index, window, cx);
                        } else {
                            pane.activate(index, window, cx);
                        }
                    }),
                )
                .on_close({
                    let pane = cx.weak_entity();
                    move |window, cx| {
                        let closed =
                            pane.update(cx, |pane, cx| pane.close_tab_at(index, window, cx));
                        if closed.is_err() {
                            tracing::debug!(
                                "pane closed before its tab's close button was handled"
                            );
                        }
                    }
                })
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .drag_over::<DraggedTab>(move |style, _, _, cx| {
                    style
                        .border_l(cx.theme().metrics.tab_indicator)
                        .border_color(cx.theme().colors.accent)
                })
                .on_drop(cx.listener(move |pane, dragged: &DraggedTab, _, cx| {
                    pane.drop_tab(dragged, index, cx);
                }))
        });
        TabBar::new("tabs")
            .tabs(tabs.collect::<Vec<_>>())
            .end_button(
                "+",
                cx.listener(|pane, _, window, cx| pane.open_tab(window, cx)),
            )
            .on_drop(cx.listener(move |pane, dragged: &DraggedTab, _, cx| {
                pane.drop_tab(dragged, last, cx);
            }))
    }
}

/// A shell that ended normally (`exit`, Ctrl+D).
fn is_clean_exit(event: &TerminalViewEvent) -> bool {
    matches!(event, TerminalViewEvent::Exited(exit) if exit.is_success())
}

/// What a tab shows: the user's name, else the program's title, else a default.
fn label(tab: &PaneTab, cx: &gpui::App) -> SharedString {
    tab.name
        .clone()
        .or_else(|| tab.view.title(cx))
        .unwrap_or_else(|| SharedString::new_static(DEFAULT_TITLE))
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.tabs.active().map(|tab| tab.view.element());
        div()
            .key_context("Pane")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(Self::quick_connect))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .size_full()
            .child(self.tab_bar(cx))
            .child(div().flex().flex_1().min_h_0().children(active).when(
                self.tabs.is_empty(),
                |d| {
                    d.items_center()
                        .justify_center()
                        .bg(theme.colors.background)
                        .text_color(theme.colors.text_muted)
                        .child(
                            div()
                                .id("empty-new-tab")
                                .px(theme.spacing.lg)
                                .py(theme.spacing.sm)
                                .rounded(theme.radius.sm)
                                .border(theme.metrics.border_width)
                                .border_color(theme.colors.border)
                                .hover(|style| style.text_color(theme.colors.text))
                                .on_click(cx.listener(|pane, _, window, cx| {
                                    pane.open_tab(window, cx);
                                }))
                                .child("New tab"),
                        )
                        .child(
                            div()
                                .id("empty-quick-connect")
                                .ml(theme.spacing.md)
                                .px(theme.spacing.lg)
                                .py(theme.spacing.sm)
                                .rounded(theme.radius.sm)
                                .border(theme.metrics.border_width)
                                .border_color(theme.colors.border)
                                .hover(|style| style.text_color(theme.colors.text))
                                .on_click(cx.listener(|pane, _, window, cx| {
                                    pane.quick_connect(&QuickConnect, window, cx);
                                }))
                                .child("Quick connect"),
                        )
                },
            ))
            .when_some(self.quick_connect.as_ref(), |d, (bar, _)| {
                d.child(bar.clone())
            })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use gpui::{TestAppContext, VisualTestContext};
    use ssherlock_terminal::{ShellConfig, ShellProgram};

    use super::*;
    use crate::terminal::DefaultShell;

    /// A pane in a test window. Its terminals are pointed at a program that doesn't exist, so
    /// no real shell is started.
    fn pane(cx: &mut TestAppContext) -> (Entity<Pane>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::keymap::init(cx);
            cx.set_global(DefaultShell(ShellConfig {
                program: ShellProgram::Command {
                    program: "/nonexistent/ssherlock-test-shell".into(),
                    args: Vec::new(),
                },
                ..ShellConfig::default()
            }));
        });
        cx.add_window_view(Pane::new)
    }

    #[gpui::test]
    fn actions_open_switch_move_and_close_tabs(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        let state = |cx: &mut VisualTestContext| {
            pane.read_with(cx, |pane, _| (pane.tab_count(), pane.active_index()))
        };
        assert_eq!(state(cx), (1, Some(0)));

        cx.dispatch_action(NewTab);
        cx.dispatch_action(NewTab);
        assert_eq!(state(cx), (3, Some(2)));

        cx.dispatch_action(NextTab);
        assert_eq!(state(cx), (3, Some(0)));
        cx.dispatch_action(PreviousTab);
        assert_eq!(state(cx), (3, Some(2)));
        cx.dispatch_action(MoveTabLeft);
        assert_eq!(state(cx), (3, Some(1)));
        cx.dispatch_action(MoveTabRight);
        assert_eq!(state(cx), (3, Some(2)));

        cx.dispatch_action(CloseTab);
        assert_eq!(state(cx), (2, Some(1)));
        cx.dispatch_action(CloseTab);
        cx.dispatch_action(CloseTab);
        assert_eq!(state(cx), (0, None));
        // Actions still reach an empty pane.
        cx.dispatch_action(NewTab);
        assert_eq!(state(cx), (1, Some(0)));
    }

    #[gpui::test]
    fn quick_connect_opens_and_dismisses(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        cx.dispatch_action(QuickConnect);
        assert!(pane.read_with(cx, |pane, _| pane.is_quick_connect_open()));
        cx.simulate_keystrokes("escape");
        assert!(!pane.read_with(cx, |pane, _| pane.is_quick_connect_open()));
        assert_eq!(pane.read_with(cx, |pane, _| pane.tab_count()), 1);
    }

    #[gpui::test]
    fn quick_connect_opens_a_remote_tab(cx: &mut TestAppContext) {
        // A real runtime: the connection attempt runs on it. Port 9 on localhost has nothing
        // listening, so the attempt fails at once and never leaves this machine.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("ssherlock-ui-qc-{}", std::process::id()));
        cx.update(|cx| {
            cx.set_global(crate::runtime::TokioHandle::new(runtime.handle().clone()));
            cx.set_global(crate::ssh::SshEnvironment {
                known_hosts: dir.join("known_hosts"),
                read_only_known_hosts: Vec::new(),
                ssh_dir: None,
                local_user: Some("tester".into()),
            });
        });
        let (pane, cx) = pane(cx);
        cx.dispatch_action(QuickConnect);
        cx.simulate_input("127.0.0.1:not-a-port");
        cx.simulate_keystrokes("enter");
        assert!(
            pane.read_with(cx, |pane, _| pane.is_quick_connect_open()),
            "invalid text keeps the bar open"
        );
        cx.simulate_keystrokes("escape");

        cx.dispatch_action(QuickConnect);
        cx.simulate_input("127.0.0.1:9");
        cx.simulate_keystrokes("enter");
        let (count, active, title) = pane.read_with(cx, |pane, cx| {
            (pane.tab_count(), pane.active_index(), pane.title(cx))
        });
        assert_eq!((count, active), (2, Some(1)));
        assert_eq!(
            title, "tester@127.0.0.1:9",
            "the user comes from the environment"
        );
        drop(runtime);
    }

    #[gpui::test]
    fn renaming_sets_and_clears_the_label(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        let title = |cx: &mut VisualTestContext| pane.read_with(cx, Pane::title);
        assert_eq!(title(cx), DEFAULT_TITLE);

        cx.dispatch_action(RenameTab);
        cx.simulate_input("prod db");
        cx.simulate_keystrokes("enter");
        assert_eq!(title(cx), "prod db");

        cx.dispatch_action(RenameTab);
        cx.simulate_keystrokes("x escape");
        assert_eq!(title(cx), "prod db", "escape keeps the old name");

        cx.dispatch_action(RenameTab);
        cx.simulate_keystrokes("backspace enter");
        assert_eq!(
            title(cx),
            DEFAULT_TITLE,
            "an empty name goes back to the default"
        );
    }
}
