//! [`Pane`]: a tab bar over the active tab's terminal.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EntityId, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::hosts::{Colour, Host};
use ssherlock_core::{ConnectTarget, Id};

use super::actions::{
    CloseTab, MoveTabLeft, MoveTabRight, NewTab, NextTab, PinTab, PreviousTab, RenameTab, ShowHosts,
};
use super::list::{Step, TabList};
use crate::components::{InputKind, Tab, TabBar, TextInput, TextInputEvent};
use crate::hosts::{ConnectRequest, HostList, HostListEvent, HostsModel};
use crate::ssh::actions::QuickConnect;
use crate::ssh::{ConnectOptions, QuickConnectBar, QuickConnectEvent, RemoteTerminal};
use crate::terminal::{TerminalView, TerminalViewEvent};
use crate::theme::ActiveTheme as _;

/// Shown for a tab whose program hasn't set a title.
const DEFAULT_TITLE: &str = "Terminal";
/// The title of the hosts page's tab, and of the window while it's shown.
const HOSTS_TITLE: &str = "Hosts";

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
    /// The saved host it's connected to, if it was opened from one.
    host: Option<Id<Host>>,
    /// The host's colour, marked on the tab (TERM-05).
    colour: Option<Colour>,
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

/// A tab bar over the active tab's terminal, with the hosts page pinned in front.
pub struct Pane {
    tabs: TabList<PaneTab>,
    rename: Option<Rename>,
    quick_connect: Option<(Entity<QuickConnectBar>, Subscription)>,
    /// The hosts page, if saved hosts are set up (`HostsModel` installed).
    hosts: Option<(Entity<HostList>, Subscription)>,
    /// Whether the hosts page is in front rather than the active tab.
    is_showing_hosts: bool,
    focus_handle: FocusHandle,
}

impl Focusable for Pane {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if let Some((bar, _)) = &self.quick_connect {
            return bar.focus_handle(cx);
        }
        if let Some(hosts) = self.visible_hosts() {
            return hosts.focus_handle(cx);
        }
        self.tabs.active().map_or_else(
            || self.focus_handle.clone(),
            |tab| tab.view.focus_handle(cx),
        )
    }
}

impl Pane {
    /// A pane with one local terminal tab, and the hosts page behind its own tab if saved
    /// hosts are set up.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hosts = HostsModel::global(cx).map(|model| {
            let list = cx.new(|cx| HostList::new(model, cx));
            let subscription = cx.subscribe_in(&list, window, Self::hosts_event);
            (list, subscription)
        });
        let mut pane = Self {
            tabs: TabList::default(),
            rename: None,
            quick_connect: None,
            hosts,
            is_showing_hosts: false,
            focus_handle: cx.focus_handle(),
        };
        pane.open_tab(window, cx);
        pane
    }

    /// The active tab's label, or "Hosts" while the hosts page is in front; for the window
    /// title.
    pub fn title(&self, cx: &gpui::App) -> SharedString {
        if self.visible_hosts().is_some() {
            return SharedString::new_static(HOSTS_TITLE);
        }
        self.tabs.active().map_or_else(
            || SharedString::new_static(DEFAULT_TITLE),
            |tab| label(tab, cx),
        )
    }

    /// The hosts page, if it's what the pane shows: chosen, or because no tab is open.
    fn visible_hosts(&self) -> Option<&Entity<HostList>> {
        self.hosts
            .as_ref()
            .filter(|_| self.is_showing_hosts || self.tabs.is_empty())
            .map(|(list, _)| list)
    }

    /// Whether the hosts page is in front.
    pub fn is_showing_hosts(&self) -> bool {
        self.visible_hosts().is_some()
    }

    fn show_hosts(&mut self, _: &ShowHosts, window: &mut Window, cx: &mut Context<Self>) {
        if self.hosts.is_none() {
            return;
        }
        // A second press goes back to the terminal.
        self.is_showing_hosts = !self.is_showing_hosts || self.tabs.is_empty();
        self.focus_active(window, cx);
    }

    fn hosts_event(
        &mut self,
        _: &Entity<HostList>,
        event: &HostListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let HostListEvent::Connect(requests) = event;
        for request in requests {
            self.open_saved_host(request, window, cx);
        }
    }

    /// Opens a tab connected to a saved host, marked with its colour, and notes the connection
    /// for Recents and the host's open sessions.
    pub fn open_saved_host(
        &mut self,
        request: &ConnectRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_remote(&request.target, &request.options, Some(request), window, cx);
        if let Some(model) = HostsModel::global(cx) {
            model.update(cx, |model, cx| {
                model.record_connection(request.host, cx);
                model.session_opened(request.host, cx);
            });
        }
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
        self.add_tab(TabView::Local(view), subscription, None, window, cx);
    }

    /// Opens a tab connected to `target` over SSH with default options, after the active one.
    pub fn open_remote_tab(
        &mut self,
        target: &ConnectTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_remote(target, &ConnectOptions::default(), None, window, cx);
    }

    fn open_remote(
        &mut self,
        target: &ConnectTarget,
        options: &ConnectOptions,
        saved: Option<&ConnectRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| RemoteTerminal::new(target, options, window, cx));
        let subscription = cx.subscribe_in(&view, window, |pane, view, event, window, cx| {
            pane.terminal_event(view.entity_id(), event, window, cx);
        });
        self.add_tab(TabView::Remote(view), subscription, saved, window, cx);
    }

    fn add_tab(
        &mut self,
        view: TabView,
        subscription: Subscription,
        saved: Option<&ConnectRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tabs.open(PaneTab {
            view,
            name: None,
            has_bell: false,
            host: saved.map(|request| request.host),
            colour: saved.and_then(|request| request.colour),
            _subscription: subscription,
        });
        self.is_showing_hosts = false;
        self.focus_active(window, cx);
    }

    fn quick_connect(&mut self, _: &QuickConnect, window: &mut Window, cx: &mut Context<Self>) {
        let bar = cx.new(|cx| QuickConnectBar::new(window, cx));
        let subscription = cx.subscribe_in(&bar, window, |pane, _, event, window, cx| {
            pane.quick_connect = None;
            match event {
                QuickConnectEvent::Connect(target) => pane.open_remote_tab(target, window, cx),
                QuickConnectEvent::ConnectSaved(request) => {
                    pane.open_saved_host(request, window, cx);
                }
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
        if let Some(host) = tab.host
            && let Some(model) = HostsModel::global(cx)
        {
            model.update(cx, |model, cx| model.session_closed(host, cx));
        }
        // Dropping the tab drops its view, which stops the terminal and hangs up the shell.
        drop(tab);
        self.focus_active(window, cx);
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(hosts) = self.visible_hosts() {
            hosts.focus_handle(cx).focus(window);
            cx.notify();
            return;
        }
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

    /// Closes the active tab, unless it's pinned (or the hosts page is in front).
    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible_hosts().is_some() {
            return;
        }
        if let Some(index) = self.tabs.active_index()
            && !self.tabs.is_pinned(index)
        {
            self.close_tab_at(index, window, cx);
        }
    }

    fn pin_tab(&mut self, _: &PinTab, _: &mut Window, cx: &mut Context<Self>) {
        if self.visible_hosts().is_none()
            && let Some(index) = self.tabs.active_index()
        {
            self.tabs.toggle_pin(index);
            cx.notify();
        }
    }

    /// Whether the active tab is pinned.
    pub fn is_active_pinned(&self) -> bool {
        self.tabs
            .active_index()
            .is_some_and(|index| self.tabs.is_pinned(index))
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        // From the hosts page, the next tab is the one that was active.
        if !std::mem::take(&mut self.is_showing_hosts) {
            self.tabs.activate_next();
        }
        self.focus_active(window, cx);
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.is_showing_hosts) {
            self.tabs.activate_previous();
        }
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
        self.is_showing_hosts = false;
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
        let hosts_in_front = self.visible_hosts().is_some();
        let hosts_tab = self.hosts.as_ref().map(|_| {
            Tab::new("hosts-tab", SharedString::new_static(HOSTS_TITLE))
                .active(hosts_in_front)
                .on_click(cx.listener(|pane, _, window, cx| {
                    pane.is_showing_hosts = true;
                    pane.focus_active(window, cx);
                }))
        });
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
                .active(!hosts_in_front && self.tabs.active_index() == Some(index))
                .indicator(tab.has_bell)
                .pinned(self.tabs.is_pinned(index))
                .mark(tab.colour.map(|colour| cx.theme().colors.marks.get(colour)))
                .on_click(
                    cx.listener(move |pane, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() == 2 {
                            pane.start_rename(index, window, cx);
                        } else {
                            pane.activate(index, window, cx);
                        }
                    }),
                )
                .when(!self.tabs.is_pinned(index), |tab| {
                    tab.on_close({
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
            .tabs(hosts_tab.into_iter().chain(tabs).collect::<Vec<_>>())
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
        let active = match self.visible_hosts() {
            Some(hosts) => Some(hosts.clone().into_any_element()),
            None => self.tabs.active().map(|tab| tab.view.element()),
        };
        let is_empty = active.is_none();
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
            .on_action(cx.listener(Self::show_hosts))
            .on_action(cx.listener(Self::pin_tab))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .size_full()
            .child(self.tab_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(active)
                    .when(is_empty, |d| {
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
                    }),
            )
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

    /// Regression: the bar didn't block the mouse, so the terminal under it saw every click
    /// on it and took focus back. Once focus had left the bar, clicking it couldn't return it.
    #[gpui::test]
    fn clicking_the_quick_connect_bar_over_a_terminal_focuses_it(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        cx.dispatch_action(QuickConnect);
        cx.run_until_parked();
        let bar_has_focus = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let pane = pane.read(cx);
                let (bar, _) = pane.quick_connect.as_ref().expect("bar is open");
                bar.focus_handle(cx).contains_focused(window, cx)
            })
        };
        assert!(bar_has_focus(cx), "the bar takes focus as it opens");

        let size = cx.update(|window, _| window.viewport_size());
        let on_terminal = gpui::point(size.width / 2., size.height * 0.75);
        cx.simulate_mouse_move(on_terminal, None, gpui::Modifiers::none());
        cx.simulate_click(on_terminal, gpui::Modifiers::none());
        assert!(!bar_has_focus(cx), "clicking the terminal focuses it");

        let field = cx
            .debug_bounds("quick-connect-input")
            .expect("bar is drawn");
        cx.simulate_mouse_move(field.center(), None, gpui::Modifiers::none());
        cx.simulate_click(field.center(), gpui::Modifiers::none());
        assert!(
            bar_has_focus(cx),
            "clicking the bar's field focuses it again"
        );
        assert!(pane.read_with(cx, |pane, _| pane.is_quick_connect_open()));
    }

    /// Regression: the terminal's find bar didn't block the mouse either, so clicking its
    /// field from the terminal left focus in the terminal.
    #[gpui::test]
    fn clicking_the_find_bar_focuses_it(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        let size = cx.update(|window, _| window.viewport_size());
        let on_terminal = gpui::point(size.width / 2., size.height * 0.75);
        let click = |at: gpui::Point<gpui::Pixels>, cx: &mut VisualTestContext| {
            cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
            cx.simulate_click(at, gpui::Modifiers::none());
        };
        click(on_terminal, cx);
        cx.dispatch_action(crate::terminal::actions::Find);
        cx.run_until_parked();
        let field = cx
            .debug_bounds("terminal-search-input")
            .expect("find bar is open");
        let field_has_focus = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let terminal = match pane.read(cx).tabs.active().map(|tab| &tab.view) {
                    Some(TabView::Local(terminal)) => Some(terminal),
                    _ => None,
                }
                .expect("the pane has a local terminal tab");
                let field = terminal.read(cx).search_focus_handle(cx);
                field.expect("find bar is open").is_focused(window)
            })
        };
        assert!(field_has_focus(cx), "Find focuses the field");

        click(on_terminal, cx);
        assert!(!field_has_focus(cx), "clicking the terminal focuses it");
        click(field.center(), cx);
        assert!(field_has_focus(cx), "clicking the field focuses it again");
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

    /// A pane with saved hosts set up over an in-memory database holding `hosts`.
    fn pane_with_hosts<'a>(
        hosts: &[ssherlock_core::hosts::Host],
        cx: &'a mut TestAppContext,
    ) -> (Entity<Pane>, Entity<HostsModel>, &'a mut VisualTestContext) {
        use ssherlock_store::Repository as _;

        cx.update(crate::notify::init);
        let store = std::sync::Arc::new(ssherlock_store::Store::open_in_memory().unwrap());
        store.upsert_all(hosts).unwrap();
        let model = cx.update(|cx| HostsModel::install(store, cx));
        cx.run_until_parked();
        let (pane, cx) = pane(cx);
        cx.run_until_parked();
        (pane, model, cx)
    }

    #[gpui::test]
    fn the_hosts_page_toggles_and_fills_an_empty_pane(cx: &mut TestAppContext) {
        let (pane, _, cx) = pane_with_hosts(&[], cx);
        let state = |cx: &mut VisualTestContext| {
            pane.read_with(cx, |pane, cx| {
                (pane.is_showing_hosts(), pane.title(cx).to_string())
            })
        };
        assert_eq!(
            state(cx),
            (false, DEFAULT_TITLE.to_owned()),
            "it starts on a terminal"
        );
        cx.dispatch_action(ShowHosts);
        assert_eq!(state(cx), (true, HOSTS_TITLE.to_owned()));
        cx.dispatch_action(ShowHosts);
        assert!(!state(cx).0, "a second press goes back");
        cx.dispatch_action(ShowHosts);
        cx.dispatch_action(NextTab);
        assert!(!state(cx).0, "the next tab leaves the hosts page");

        cx.dispatch_action(CloseTab);
        assert!(state(cx).0, "with no tabs left, the hosts page shows");
        cx.dispatch_action(ShowHosts);
        assert!(state(cx).0, "and stays, as there's nothing to go back to");
        cx.dispatch_action(NewTab);
        assert!(!state(cx).0, "a new tab comes to the front");
    }

    #[gpui::test]
    fn connecting_to_a_saved_host_from_the_keyboard(cx: &mut TestAppContext) {
        // Port 9 on localhost has nothing listening, so the attempt fails at once and never
        // leaves this machine.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("ssherlock-ui-hosts-{}", std::process::id()));
        cx.update(|cx| {
            cx.set_global(crate::runtime::TokioHandle::new(runtime.handle().clone()));
            cx.set_global(crate::ssh::SshEnvironment {
                known_hosts: dir.join("known_hosts"),
                read_only_known_hosts: Vec::new(),
                ssh_dir: None,
                local_user: Some("tester".into()),
            });
        });
        let mut host = ssherlock_core::hosts::Host::new(
            "127.0.0.1".parse().unwrap(),
            ssherlock_core::Timestamp::UNIX_EPOCH,
        );
        host.label = "local test".into();
        host.settings.port = ssherlock_core::Port::new(9);
        host.settings.username = Some("saved-user".into());
        host.settings.colour = Some(ssherlock_core::hosts::Colour::Teal);
        let id = host.id;
        let (pane, model, cx) = pane_with_hosts(&[host], cx);

        cx.dispatch_action(ShowHosts);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let (count, showing, title) = pane.read_with(cx, |pane, cx| {
            (pane.tab_count(), pane.is_showing_hosts(), pane.title(cx))
        });
        assert_eq!((count, showing), (2, false));
        assert_eq!(
            title, "saved-user@127.0.0.1:9",
            "the saved user and port are used"
        );
        let last_connected =
            model.read_with(cx, |m, _| m.host(id).and_then(|h| h.last_connected_at));
        assert!(
            last_connected.is_some(),
            "the connection is recorded for Recents"
        );
        let colour = pane.read_with(cx, |pane, _| pane.tabs.active().and_then(|tab| tab.colour));
        assert_eq!(
            colour,
            Some(ssherlock_core::hosts::Colour::Teal),
            "the tab carries the host's colour"
        );
        assert_eq!(model.read_with(cx, |m, _| m.open_sessions(id)), 1);
        cx.dispatch_action(CloseTab);
        assert_eq!(
            model.read_with(cx, |m, _| m.open_sessions(id)),
            0,
            "closing the tab ends the session"
        );
        drop(runtime);
    }

    #[gpui::test]
    fn a_pinned_tab_stays_open_until_unpinned(cx: &mut TestAppContext) {
        let (pane, cx) = pane(cx);
        cx.dispatch_action(NewTab);
        cx.dispatch_action(PinTab);
        let state = |cx: &mut VisualTestContext| {
            pane.read_with(cx, |pane, _| {
                (
                    pane.tab_count(),
                    pane.active_index(),
                    pane.is_active_pinned(),
                )
            })
        };
        assert_eq!(
            state(cx),
            (2, Some(0), true),
            "pinning moves the tab to the front"
        );
        cx.dispatch_action(CloseTab);
        assert_eq!(
            state(cx),
            (2, Some(0), true),
            "Close does nothing to a pinned tab"
        );
        cx.dispatch_action(PinTab);
        cx.dispatch_action(CloseTab);
        assert_eq!(state(cx).0, 1, "once unpinned it closes");
    }
}
