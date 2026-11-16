//! [`HostList`]: the hosts page. A navigation column (all, favourites, recent, groups, tags,
//! trash), a search field, the hosts as a list or grid, and the details of the host under the
//! cursor (HOST-03…07, 09, 12…14).
//!
//! It holds view state only: what's selected, what's searched, how it's sorted. The hosts
//! themselves come from the shared [`HostsModel`], and every change goes back through it.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div,
};
use ssherlock_core::hosts::search::{self, HostQuery, Scope, SortOrder};
use ssherlock_core::hosts::{BulkEdit, Change, Group, GroupTree, Host, resolve};
use ssherlock_core::{ConnectTarget, Id, Timestamp, time};

use super::actions::{
    ChangeSort, ClearSearch, ClearSelection, CollapseGroup, ConnectSelected, DeleteForever,
    DuplicateSelected, EditSelected, EmptyTrash, ExpandGroup, ExtendNext, ExtendPrevious,
    ExtendToFirst, ExtendToLast, FocusList, FocusSearch, MoveToGroup, NewGroup, NewHost, Restore,
    SelectAll, SelectFirst, SelectLast, SelectNext, SelectPrevious, ToggleFavourite, ToggleLayout,
    ToggleSelected, TrashGroup, TrashSelected,
};
use super::connect::{ConnectRequest, target_text};
use super::detail::{CaseFile, case_file};
use super::editor::{EditorMode, HostEditor, HostEditorEvent, Saved};
use super::item::{HostItem, HostSummary, Layout, mark};
use super::model::{HostsModel, LoadState};
use super::nav::{self, NavItem, Section};
use super::selection::Selection;
use crate::components::form::{FocusNext, FocusPrevious};
use crate::components::{
    Button, ButtonStyle, Picker, PickerEvent, PickerItem, TextInput, TextInputEvent,
};
use crate::ssh::SshEnvironment;
use crate::theme::{ActiveTheme as _, Theme};

/// How many recent hosts the strip above "All hosts" shows (APP-16).
const RECENT_STRIP: usize = 5;

/// What the hosts page asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostListEvent {
    /// Open a tab connected to each of these, in order.
    Connect(Vec<ConnectRequest>),
}

/// The hosts page.
pub struct HostList {
    model: Entity<HostsModel>,
    search: Entity<TextInput>,
    section: Section,
    sort: SortOrder,
    layout: Layout,
    expanded: HashSet<Id<Group>>,
    selection: Selection,
    /// The hosts shown, filtered and sorted.
    visible: Vec<Host>,
    /// Index of the trash entry under the cursor.
    trash_cursor: usize,
    /// A picker open over the page (sort order, or a group to move hosts to).
    picker: Option<(Entity<Picker>, Subscription)>,
    editor: Option<(Entity<HostEditor>, Subscription)>,
    /// A host just saved, to put the cursor on once it shows up.
    pending_cursor: Option<Id<Host>>,
    nav_focus: FocusHandle,
    list_focus: FocusHandle,
    list_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HostListEvent> for HostList {}

impl Focusable for HostList {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if let Some((editor, _)) = &self.editor {
            return editor.focus_handle(cx);
        }
        match &self.picker {
            Some((picker, _)) => picker.focus_handle(cx),
            None => self.list_focus.clone(),
        }
    }
}

impl HostList {
    /// The page over `model`.
    pub fn new(model: Entity<HostsModel>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::field("", cx).placeholder("Search hosts"));
        let subscriptions = vec![
            cx.observe(&model, |list, _, cx| list.refresh(cx)),
            cx.subscribe(&search, |list, _, event: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(_) = event {
                    list.refresh(cx);
                }
            }),
        ];
        let mut list = Self {
            model,
            search,
            section: Section::all(),
            sort: SortOrder::default(),
            layout: Layout::default(),
            expanded: HashSet::new(),
            selection: Selection::default(),
            visible: Vec::new(),
            trash_cursor: 0,
            picker: None,
            editor: None,
            pending_cursor: None,
            nav_focus: cx.focus_handle().tab_stop(true),
            list_focus: cx.focus_handle().tab_stop(true),
            list_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        list.refresh(cx);
        list
    }

    /// The ids of the hosts shown, in order.
    pub fn visible_ids(&self) -> Vec<Id<Host>> {
        self.visible.iter().map(|h| h.id).collect()
    }

    /// The part of the page shown.
    pub fn section(&self) -> &Section {
        &self.section
    }

    /// The selection and cursor.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Recomputes what's shown after the hosts, the search, the section or the sort changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let old_ids = self.visible_ids();
        let old_position = self.selection.position(&old_ids);
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        // A group or tag that's gone (deleted, or its last host untagged) falls back to all.
        let still_there = match &self.section {
            Section::Hosts(Scope::Group(id)) => tree.get(*id).is_some(),
            Section::Tag(tag) => model.hosts().iter().any(|h| h.tags.contains(tag)),
            Section::Hosts(_) | Section::Trash => true,
        };
        if !still_there {
            self.section = Section::all();
        }
        let query = match &self.section {
            Section::Hosts(scope) => HostQuery {
                text: self.search.read(cx).text().to_owned(),
                tag: None,
                scope: *scope,
            },
            Section::Tag(tag) => HostQuery {
                text: self.search.read(cx).text().to_owned(),
                tag: Some(tag.clone()),
                scope: Scope::All,
            },
            Section::Trash => HostQuery::default(),
        };
        self.visible = if self.section == Section::Trash {
            Vec::new()
        } else {
            search::filter(model.hosts(), &query, &tree, self.sort)
                .into_iter()
                .cloned()
                .collect()
        };
        self.trash_cursor = self.trash_cursor.min(model.trash().len().saturating_sub(1));
        let ids = self.visible_ids();
        self.selection.retain(&ids, old_position);
        if let Some(id) = self.pending_cursor
            && ids.contains(&id)
        {
            self.pending_cursor = None;
            self.selection.click(id);
            self.after_cursor_move(cx);
        }
        cx.notify();
    }

    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.selection = Selection::default();
            self.trash_cursor = 0;
            self.refresh(cx);
        }
    }

    // ── navigation column ──────────────────────────────────────────────────────────────

    fn nav_items(&self, cx: &App) -> Vec<NavItem> {
        let model = self.model.read(cx);
        nav::items(model.hosts(), model.groups(), model.trash().len(), |id| {
            self.expanded.contains(&id)
        })
    }

    /// Moves the navigation highlight (which is the section shown) to the first or last entry.
    fn nav_jump(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let items = self.nav_items(cx);
        let mut sections = items.iter().filter_map(NavItem::section);
        let target = if to_end {
            sections.next_back()
        } else {
            sections.next()
        };
        if let Some(section) = target.cloned() {
            self.show(section, cx);
        }
    }

    fn nav_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_jump(false, cx);
    }

    fn nav_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_jump(true, cx);
    }

    /// Moves the navigation highlight (which is the section shown) by one entry.
    fn nav_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let sections: Vec<Section> = self
            .nav_items(cx)
            .iter()
            .filter_map(NavItem::section)
            .cloned()
            .collect();
        let Some(last) = sections.len().checked_sub(1) else {
            return;
        };
        let at = sections.iter().position(|s| *s == self.section);
        let next = match (at, forward) {
            (Some(at), true) => (at + 1).min(last),
            (Some(at), false) => at.saturating_sub(1),
            (None, _) => 0,
        };
        if let Some(section) = sections.get(next).cloned() {
            self.show(section, cx);
        }
    }

    fn current_group(&self) -> Option<Id<Group>> {
        match self.section {
            Section::Hosts(Scope::Group(id)) => Some(id),
            _ => None,
        }
    }

    fn expand_group(&mut self, _: &ExpandGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.current_group()
            && self.expanded.insert(id)
        {
            cx.notify();
        }
    }

    /// Collapses the group, or if it's already collapsed, moves up to its parent.
    fn collapse_group(&mut self, _: &CollapseGroup, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current_group() else {
            return;
        };
        if self.expanded.remove(&id) {
            cx.notify();
            return;
        }
        let parent = self
            .model
            .read(cx)
            .groups()
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.parent);
        if let Some(parent) = parent {
            self.show(Section::Hosts(Scope::Group(parent)), cx);
        }
    }

    fn toggle_expanded(&mut self, id: Id<Group>, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        cx.notify();
    }

    fn nav_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_step(true, cx);
    }

    fn nav_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_step(false, cx);
    }

    // ── list ───────────────────────────────────────────────────────────────────────────

    fn after_cursor_move(&mut self, cx: &mut Context<Self>) {
        if let Some(position) = self.selection.position(&self.visible_ids()) {
            self.list_scroll.scroll_to_item(position);
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.section == Section::Trash {
            let count = self.model.read(cx).trash().len();
            if let Some(last) = count.checked_sub(1) {
                self.trash_cursor = self.trash_cursor.saturating_add_signed(delta).min(last);
                self.list_scroll.scroll_to_item(self.trash_cursor);
                cx.notify();
            }
            return;
        }
        let ids = self.visible_ids();
        self.selection.step(&ids, delta, extend);
        self.after_cursor_move(cx);
    }

    fn jump(&mut self, to_end: bool, extend: bool, cx: &mut Context<Self>) {
        if self.section == Section::Trash {
            let count = self.model.read(cx).trash().len();
            self.trash_cursor = if to_end { count.saturating_sub(1) } else { 0 };
            self.list_scroll.scroll_to_item(self.trash_cursor);
            cx.notify();
            return;
        }
        let ids = self.visible_ids();
        self.selection.jump(&ids, to_end, extend);
        self.after_cursor_move(cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, false, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, false, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, false, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, false, cx);
    }

    fn extend_next(&mut self, _: &ExtendNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, true, cx);
    }

    fn extend_previous(&mut self, _: &ExtendPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, true, cx);
    }

    fn extend_to_first(&mut self, _: &ExtendToFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, true, cx);
    }

    fn extend_to_last(&mut self, _: &ExtendToLast, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, true, cx);
    }

    fn toggle_selected(&mut self, _: &ToggleSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cursor) = self.selection.cursor() {
            self.selection.toggle(cursor);
            cx.notify();
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.visible_ids();
        self.selection.select_all(&ids);
        cx.notify();
    }

    fn clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.clear();
        cx.notify();
    }

    fn targets(&self) -> Vec<Id<Host>> {
        self.selection.targets(&self.visible_ids())
    }

    fn connect_selected(&mut self, _: &ConnectSelected, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        self.connect(&targets, cx);
    }

    /// Asks the owner to connect to `ids`, resolved against their groups.
    fn connect(&mut self, ids: &[Id<Host>], cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let requests: Vec<ConnectRequest> = ids
            .iter()
            .filter_map(|&id| model.host(id))
            .map(|host| ConnectRequest::new(host, model.groups(), model.identities()))
            .collect();
        if !requests.is_empty() {
            cx.emit(HostListEvent::Connect(requests));
        }
    }

    fn toggle_favourite(&mut self, _: &ToggleFavourite, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        // Mixed selections become favourites; only an all-favourite selection is unmarked.
        let all_favourite = targets
            .iter()
            .all(|id| self.visible.iter().any(|h| h.id == *id && h.is_favourite));
        self.model.update(cx, |model, cx| {
            model.set_favourite(&targets, !all_favourite, cx);
        });
    }

    fn trash_selected(&mut self, _: &TrashSelected, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        if !targets.is_empty() {
            self.model
                .update(cx, |model, cx| model.trash_hosts(targets, cx));
        }
    }

    fn toggle_layout(&mut self, _: &ToggleLayout, _: &mut Window, cx: &mut Context<Self>) {
        self.layout = match self.layout {
            Layout::List => Layout::Grid,
            Layout::Grid => Layout::List,
        };
        cx.notify();
    }

    fn change_sort(&mut self, _: &ChangeSort, window: &mut Window, cx: &mut Context<Self>) {
        let items = SortOrder::ALL
            .iter()
            .map(|order| PickerItem::new(order.label()))
            .collect();
        let current = SortOrder::ALL
            .iter()
            .position(|o| *o == self.sort)
            .unwrap_or(0);
        let picker = cx.new(|cx| Picker::new("Sort by", items, window, cx).with_selected(current));
        let subscription = cx.subscribe_in(&picker, window, |list, _, event, window, cx| {
            if let PickerEvent::Picked(index) = event
                && let Some(order) = SortOrder::ALL.get(*index)
            {
                list.sort = *order;
                list.refresh(cx);
            }
            list.picker = None;
            list.list_focus.focus(window);
            cx.notify();
        });
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.focus_handle(cx).focus(window);
    }

    fn focus_list(&mut self, _: &FocusList, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window);
        if self.selection.cursor().is_none() {
            let ids = self.visible_ids();
            self.selection.step(&ids, 1, false);
        }
        cx.notify();
    }

    fn clear_search(&mut self, _: &ClearSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.list_focus.focus(window);
    }

    // ── editing ────────────────────────────────────────────────────────────────────────

    fn open_editor(&mut self, mode: EditorMode, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let editor = cx.new(|cx| HostEditor::new(model, mode, window, cx));
        let subscription = cx.subscribe_in(&editor, window, |list, _, event, window, cx| {
            let HostEditorEvent::Done(saved) = event;
            if let Some(Saved::Host(id)) = saved {
                list.pending_cursor = Some(*id);
            }
            if let Some(Saved::Group(id)) = saved {
                list.expanded.extend(list.parent_of(*id, cx));
            }
            list.editor = None;
            list.list_focus.focus(window);
            cx.notify();
        });
        self.editor = Some((editor, subscription));
        cx.notify();
    }

    fn parent_of(&self, id: Id<Group>, cx: &App) -> Option<Id<Group>> {
        self.model
            .read(cx)
            .groups()
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.parent)
    }

    fn new_host(&mut self, _: &NewHost, window: &mut Window, cx: &mut Context<Self>) {
        let group = self.current_group();
        self.open_editor(EditorMode::NewHost { group }, window, cx);
    }

    fn new_group(&mut self, _: &NewGroup, window: &mut Window, cx: &mut Context<Self>) {
        let parent = self.current_group();
        self.open_editor(EditorMode::NewGroup { parent }, window, cx);
    }

    /// Edits the host under the cursor, or all the selected hosts at once.
    fn edit_selected(&mut self, _: &EditSelected, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        let mode = match targets.as_slice() {
            [] => return,
            [id] => match self.model.read(cx).host(*id) {
                Some(host) => EditorMode::EditHost(host.clone()),
                None => return,
            },
            _ => EditorMode::Bulk(targets),
        };
        self.open_editor(mode, window, cx);
    }

    /// Edits the group selected in the navigation column.
    fn edit_group(&mut self, _: &EditSelected, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current_group() else {
            return;
        };
        let group = self
            .model
            .read(cx)
            .groups()
            .iter()
            .find(|g| g.id == id)
            .cloned();
        if let Some(group) = group {
            self.open_editor(EditorMode::EditGroup(group), window, cx);
        }
    }

    fn trash_group(&mut self, _: &TrashGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.current_group() {
            self.model.update(cx, |model, cx| model.trash_group(id, cx));
        }
    }

    fn duplicate_selected(
        &mut self,
        _: &DuplicateSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Timestamp::now();
        let model = self.model.read(cx);
        let copies: Vec<Host> = self
            .targets()
            .iter()
            .filter_map(|id| model.host(*id))
            .map(|host| host.duplicate(now))
            .collect();
        self.pending_cursor = copies.first().map(|h| h.id);
        if !copies.is_empty() {
            self.model
                .update(cx, |model, cx| model.save_hosts(copies, cx));
        }
    }

    /// Opens a picker to move the selected hosts into a group (HOST-08).
    fn move_to_group(&mut self, _: &MoveToGroup, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        let mut choices: Vec<(Option<Id<Group>>, String)> = vec![(None, "No group".to_owned())];
        choices.extend(
            tree.rows(|_| true)
                .into_iter()
                .map(|row| (Some(row.group.id), tree.path(row.group.id))),
        );
        let items = choices
            .iter()
            .map(|(_, label)| PickerItem::new(label.clone()))
            .collect();
        let placeholder = match targets.len() {
            1 => "Move to group".to_owned(),
            n => format!("Move {n} hosts to group"),
        };
        let picker = cx.new(|cx| Picker::new(placeholder, items, window, cx).creating("group"));
        let subscription = cx.subscribe_in(&picker, window, move |list, _, event, window, cx| {
            let group = match event {
                PickerEvent::Picked(index) => choices.get(*index).map(|(group, _)| *group),
                // A new top-level group, saved before the hosts move into it.
                PickerEvent::Create(name) => {
                    let group = Group::new(name.as_str(), None, Timestamp::now());
                    match group.validate() {
                        Ok(()) => {
                            let id = group.id;
                            list.model
                                .update(cx, |model, cx| model.save_group(group, cx));
                            Some(Some(id))
                        }
                        Err(error) => {
                            crate::notify::error(&error, cx);
                            None
                        }
                    }
                }
                PickerEvent::Dismissed => None,
            };
            if let Some(group) = group {
                let edit = BulkEdit {
                    group: Change::Set(group),
                    ..BulkEdit::default()
                };
                let now = Timestamp::now();
                let model = list.model.read(cx);
                let moved: Vec<Host> = targets
                    .iter()
                    .filter_map(|id| model.host(*id))
                    .map(|host| edit.apply(host, now))
                    .collect();
                list.model
                    .update(cx, |model, cx| model.save_hosts(moved, cx));
            }
            list.picker = None;
            list.list_focus.focus(window);
            cx.notify();
        });
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    // ── trash ──────────────────────────────────────────────────────────────────────────

    fn trash_entry_id(&self, cx: &App) -> Option<Id<ssherlock_core::trash::TrashEntry>> {
        self.model
            .read(cx)
            .trash()
            .get(self.trash_cursor)
            .map(|e| e.id)
    }

    fn restore(&mut self, _: &Restore, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.trash_entry_id(cx) {
            self.model.update(cx, |model, cx| model.restore(id, cx));
        }
    }

    fn delete_forever(&mut self, _: &DeleteForever, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.trash_entry_id(cx) {
            self.model
                .update(cx, |model, cx| model.delete_forever(id, cx));
        }
    }

    fn empty_trash(&mut self, _: &EmptyTrash, _: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, HostsModel::empty_trash);
    }

    // ── mouse ──────────────────────────────────────────────────────────────────────────

    fn click_host(&mut self, id: Id<Host>, event: &ClickEvent, cx: &mut Context<Self>) {
        let modifiers = event.modifiers();
        if event.click_count() == 2 {
            self.selection.click(id);
            self.connect(&[id], cx);
        } else if modifiers.shift {
            let ids = self.visible_ids();
            self.selection.extend_to(&ids, id);
        } else if modifiers.secondary() {
            self.selection.toggle(id);
        } else {
            self.selection.click(id);
        }
        cx.notify();
    }
}

impl Render for HostList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let state = self.model.read(cx).state().clone();
        let body: AnyElement = match state {
            LoadState::Loading => message(&theme, "Loading saved hosts\u{2026}").into_any_element(),
            LoadState::Failed(reason) => {
                message(&theme, format!("Saved hosts are unavailable: {reason}")).into_any_element()
            }
            LoadState::Ready => self.render_ready(&theme, window, cx).into_any_element(),
        };
        div()
            .key_context("HostsPage")
            .on_action(|_: &FocusNext, window, _| window.focus_next())
            .on_action(|_: &FocusPrevious, window, _| window.focus_prev())
            .relative()
            .flex()
            .size_full()
            .bg(theme.colors.background)
            .text_size(theme.text.ui_size)
            .text_color(theme.colors.text)
            .child(body)
            .when_some(self.picker.as_ref(), |d, (picker, _)| {
                d.child(picker.clone())
            })
            .when_some(self.editor.as_ref(), |d, (editor, _)| {
                d.child(editor.clone())
            })
    }
}

impl HostList {
    fn render_ready(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let now = Timestamp::now();
        let local_user = cx
            .try_global::<SshEnvironment>()
            .and_then(|env| env.local_user.clone());
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        let summarise = |host: &Host| {
            let effective = resolve(host, &tree.ancestors(host.group), model.identities());
            let target = ConnectTarget {
                user: effective.settings.username.value.clone(),
                address: effective.address.clone(),
                port: Some(effective.settings.port.value),
            };
            HostSummary::new(
                host,
                target_text(&target, local_user.as_deref()),
                effective.settings.colour.value,
            )
        };

        // The navigation column.
        let nav_focused = self.nav_focus.contains_focused(window, cx);
        let nav_rows = self
            .nav_items(cx)
            .into_iter()
            .enumerate()
            .map(|(index, item)| match item {
                NavItem::Heading(text) => div()
                    .pt(theme.spacing.lg)
                    .pb(theme.spacing.xs)
                    .px(theme.spacing.md)
                    .text_size(theme.text.small_size)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.colors.text_muted)
                    .child(text.to_uppercase())
                    .into_any_element(),
                NavItem::Entry {
                    section,
                    label,
                    count,
                    depth,
                    expanded,
                    colour,
                } => {
                    let is_current = section == self.section;
                    let hover = theme.colors.control_hover;
                    let group = match &section {
                        Section::Hosts(Scope::Group(id)) => Some(*id),
                        _ => None,
                    };
                    div()
                        .id(("nav", index))
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .pl(theme.spacing.md
                            + theme.spacing.lg * f32::from(u8::try_from(depth).unwrap_or(u8::MAX)))
                        .pr(theme.spacing.md)
                        .py(theme.spacing.sm)
                        .rounded(theme.radius.sm)
                        .cursor_pointer()
                        .when(is_current, |d| {
                            d.bg(if nav_focused {
                                theme.colors.row_selected
                            } else {
                                theme.colors.control_hover
                            })
                        })
                        .when(!is_current, |d| d.hover(move |s| s.bg(hover)))
                        .on_click(cx.listener(move |list, event: &ClickEvent, window, cx| {
                            list.show(section.clone(), cx);
                            // Double-clicking a group edits it.
                            if event.click_count() == 2 && group.is_some() {
                                list.edit_group(&EditSelected, window, cx);
                            }
                        }))
                        .child(
                            div()
                                .id(("nav-arrow", index))
                                .flex_none()
                                .w(theme.spacing.lg)
                                .text_color(theme.colors.text_muted)
                                .when_some(expanded.zip(group), |d, (is_open, id)| {
                                    d.child(if is_open { "\u{25be}" } else { "\u{25b8}" })
                                        .on_click(cx.listener(move |list, _, _, cx| {
                                            cx.stop_propagation();
                                            list.toggle_expanded(id, cx);
                                        }))
                                }),
                        )
                        .child(mark(theme, colour))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .child(
                            div()
                                .text_size(theme.text.small_size)
                                .text_color(theme.colors.text_muted)
                                .child(count.to_string()),
                        )
                        .into_any_element()
                }
            });
        let nav = div()
            .id("hosts-nav")
            .key_context("HostNav")
            .track_focus(&self.nav_focus)
            .on_action(cx.listener(Self::nav_next))
            .on_action(cx.listener(Self::nav_previous))
            .on_action(cx.listener(Self::nav_first))
            .on_action(cx.listener(Self::nav_last))
            .on_action(cx.listener(Self::expand_group))
            .on_action(cx.listener(Self::collapse_group))
            .on_action(cx.listener(Self::new_host))
            .on_action(cx.listener(Self::new_group))
            .on_action(cx.listener(Self::edit_group))
            .on_action(cx.listener(Self::trash_group))
            .on_action(cx.listener(Self::focus_list))
            .on_action(cx.listener(Self::focus_search))
            .flex()
            .flex_col()
            .flex_none()
            .w(theme.metrics.nav_width)
            .h_full()
            .overflow_y_scroll()
            .p(theme.spacing.md)
            .bg(theme.colors.surface)
            .border_r(theme.metrics.border_width)
            .border_color(if nav_focused {
                theme.colors.accent
            } else {
                theme.colors.border
            })
            .children(nav_rows);

        // The toolbar.
        let is_trash = self.section == Section::Trash;
        let toolbar = div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .child(
                div()
                    .key_context("HostSearch")
                    .on_action(cx.listener(Self::focus_list))
                    .on_action(cx.listener(Self::clear_search))
                    .flex_1()
                    .min_w_0()
                    .child(self.search.clone()),
            )
            .child(
                Button::new("new-host", "New host")
                    .style(ButtonStyle::Primary)
                    .on_click(
                        cx.listener(|list, _, window, cx| list.new_host(&NewHost, window, cx)),
                    ),
            )
            .child(
                Button::new("new-group", "New group").on_click(
                    cx.listener(|list, _, window, cx| list.new_group(&NewGroup, window, cx)),
                ),
            )
            .child(
                Button::new("sort", format!("Sort: {}", self.sort.label())).on_click(
                    cx.listener(|list, _, window, cx| list.change_sort(&ChangeSort, window, cx)),
                ),
            )
            .child(
                Button::new(
                    "layout",
                    match self.layout {
                        Layout::List => "Grid view",
                        Layout::Grid => "List view",
                    },
                )
                .on_click(cx.listener(|list, _, window, cx| {
                    list.toggle_layout(&ToggleLayout, window, cx);
                })),
            );

        // The heading over the list.
        let title = match &self.section {
            Section::Hosts(Scope::All) => "All hosts".to_owned(),
            Section::Hosts(Scope::Favourites) => "Favourites".to_owned(),
            Section::Hosts(Scope::Recent) => "Recent".to_owned(),
            Section::Hosts(Scope::Group(id)) => tree.path(*id),
            Section::Tag(tag) => format!("#{tag}"),
            Section::Trash => "Trash".to_owned(),
        };
        let count = if is_trash {
            model.trash().len()
        } else {
            self.visible.len()
        };
        let selected = self.selection.count();
        let heading = div()
            .flex()
            .items_baseline()
            .gap(theme.spacing.md)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .text_size(theme.text.small_size)
                    .text_color(theme.colors.text_muted)
                    .child(if selected > 0 {
                        format!("{count} \u{00b7} {selected} selected")
                    } else {
                        count.to_string()
                    }),
            )
            .when(is_trash && count > 0, |d| {
                d.child(div().flex_1()).child(
                    Button::new("empty-trash", "Empty trash")
                        .style(ButtonStyle::Danger)
                        .on_click(cx.listener(|list, _, window, cx| {
                            list.empty_trash(&EmptyTrash, window, cx);
                        })),
                )
            })
            // A group's own buttons; E and Delete do the same from the navigation column.
            .when(self.current_group().is_some(), |d| {
                d.child(div().flex_1())
                    .child(
                        Button::new("edit-group", "Edit group").on_click(cx.listener(
                            |list, _, window, cx| list.edit_group(&EditSelected, window, cx),
                        )),
                    )
                    .child(
                        Button::new("trash-group", "Delete group").on_click(cx.listener(
                            |list, _, window, cx| {
                                list.trash_group(&TrashGroup, window, cx);
                            },
                        )),
                    )
            });

        // Recently used hosts, on the "All hosts" page with no search (APP-16).
        let search_empty = self.search.read(cx).text().is_empty();
        let recent: Vec<(Id<Host>, SharedString)> =
            if self.section == Section::all() && search_empty {
                search::recent(model.hosts(), RECENT_STRIP)
                    .into_iter()
                    .map(|h| (h.id, SharedString::from(h.title().into_owned())))
                    .collect()
            } else {
                Vec::new()
            };
        let recent_strip = (!recent.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(theme.spacing.sm)
                .child(
                    div()
                        .text_size(theme.text.small_size)
                        .text_color(theme.colors.text_muted)
                        .child("Recent:"),
                )
                .children(recent.into_iter().enumerate().map(|(index, (id, title))| {
                    let hover = theme.colors.control_hover;
                    div()
                        .id(("recent", index))
                        .px(theme.spacing.md)
                        .py(theme.spacing.xs)
                        .rounded(theme.radius.sm)
                        .border(theme.metrics.border_width)
                        .border_color(theme.colors.border)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |list, _, _, cx| list.connect(&[id], cx)))
                        .child(title)
                }))
        });

        // The list itself.
        let list_focused = self.list_focus.contains_focused(window, cx);
        let total_hosts = model.hosts().len();
        let content: AnyElement = if is_trash {
            let entries = model.trash();
            if entries.is_empty() {
                message(theme, "The trash is empty").into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .children(entries.iter().enumerate().map(|(index, entry)| {
                        let is_cursor = index == self.trash_cursor;
                        let id = entry.id;
                        div()
                            .id(("trash", index))
                            .flex()
                            .items_center()
                            .gap(theme.spacing.lg)
                            .px(theme.spacing.md)
                            .py(theme.spacing.sm)
                            .rounded(theme.radius.sm)
                            .border(theme.metrics.border_width)
                            .border_color(if is_cursor && list_focused {
                                theme.colors.accent
                            } else if is_cursor {
                                theme.colors.border
                            } else {
                                gpui::transparent_black()
                            })
                            .on_click(cx.listener(move |list, _, _, cx| {
                                list.trash_cursor = index;
                                cx.notify();
                            }))
                            .child(div().flex_1().min_w_0().child(entry.title()))
                            .child(
                                div()
                                    .text_size(theme.text.small_size)
                                    .text_color(theme.colors.text_muted)
                                    .child(format!("deleted {}", time::ago(entry.deleted_at, now))),
                            )
                            .child(Button::new(("restore", index), "Restore").on_click(
                                cx.listener(move |list, _, _, cx| {
                                    list.model.update(cx, |model, cx| model.restore(id, cx));
                                }),
                            ))
                            .child(Button::new(("delete", index), "Delete forever").on_click(
                                cx.listener(move |list, _, _, cx| {
                                    list.model
                                        .update(cx, |model, cx| model.delete_forever(id, cx));
                                }),
                            ))
                    }))
                    .into_any_element()
            }
        } else if self.visible.is_empty() {
            let text = if total_hosts == 0 {
                "No saved hosts yet. Press Mod+O to quick connect.".to_owned()
            } else if !search_empty {
                format!(
                    "No hosts match \u{201c}{}\u{201d}",
                    self.search.read(cx).text()
                )
            } else {
                "No hosts here".to_owned()
            };
            message(theme, text).into_any_element()
        } else {
            let cursor = self.selection.cursor();
            let items = self.visible.iter().enumerate().map(|(index, host)| {
                let id = host.id;
                HostItem::new(("host", index), summarise(host), self.layout)
                    .selected(self.selection.is_selected(id))
                    .cursor(cursor == Some(id), list_focused)
                    .on_click(cx.listener(move |list, event: &ClickEvent, _, cx| {
                        list.click_host(id, event, cx);
                    }))
            });
            match self.layout {
                Layout::List => div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.xs)
                    .children(items)
                    .into_any_element(),
                Layout::Grid => div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.spacing.md)
                    .children(items)
                    .into_any_element(),
            }
        };
        let list = div()
            .id("hosts-list")
            .key_context(if is_trash { "Trash" } else { "HostList" })
            .track_focus(&self.list_focus)
            .track_scroll(&self.list_scroll)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::extend_next))
            .on_action(cx.listener(Self::extend_previous))
            .on_action(cx.listener(Self::extend_to_first))
            .on_action(cx.listener(Self::extend_to_last))
            .on_action(cx.listener(Self::toggle_selected))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::connect_selected))
            .on_action(cx.listener(Self::toggle_favourite))
            .on_action(cx.listener(Self::trash_selected))
            .on_action(cx.listener(Self::change_sort))
            .on_action(cx.listener(Self::toggle_layout))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::new_host))
            .on_action(cx.listener(Self::new_group))
            .on_action(cx.listener(Self::edit_selected))
            .on_action(cx.listener(Self::duplicate_selected))
            .on_action(cx.listener(Self::move_to_group))
            .on_action(cx.listener(Self::restore))
            .on_action(cx.listener(Self::delete_forever))
            .on_action(cx.listener(Self::empty_trash))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(content);

        // The details of the host under the cursor.
        let detail = (!is_trash)
            .then(|| self.selection.cursor())
            .flatten()
            .and_then(|id| self.visible.iter().find(|h| h.id == id))
            .map(|host| {
                let group = host
                    .group
                    .filter(|g| tree.get(*g).is_some())
                    .map(|g| tree.path(g));
                let file =
                    CaseFile::new(host, summarise(host), group, model.open_sessions(host.id));
                let id = host.id;
                case_file(
                    file,
                    theme,
                    now,
                    cx.listener(move |list, _, _, cx| list.connect(&[id], cx)),
                    cx.listener(move |list, _, window, cx| {
                        list.selection.click(id);
                        list.edit_selected(&EditSelected, window, cx);
                    }),
                )
            });

        div()
            .flex()
            .size_full()
            .child(nav)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.spacing.lg)
                    .p(theme.spacing.xl)
                    // Search, sort and layout don't apply to the trash.
                    .when(!is_trash, |d| d.child(toolbar))
                    .children(recent_strip)
                    .child(heading)
                    .child(list),
            )
            .children(detail)
    }
}

/// A faded line of text in the middle of the page.
fn message(theme: &Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .p(theme.spacing.xl)
        .text_color(theme.colors.text_muted)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui::{TestAppContext, VisualTestContext};
    use pretty_assertions::assert_eq;
    use ssherlock_core::Port;
    use ssherlock_core::hosts::Tag;
    use ssherlock_store::{Repository as _, Store};

    use super::*;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    struct Fixture {
        list: Entity<HostList>,
        model: Entity<HostsModel>,
        events: Rc<RefCell<Vec<HostListEvent>>>,
        _subscription: Subscription,
    }

    /// A page over `hosts` and `groups`, focused on the list.
    fn page<'a>(
        hosts: &[Host],
        groups: &[Group],
        cx: &'a mut TestAppContext,
    ) -> (Fixture, &'a mut VisualTestContext) {
        cx.update(|cx| {
            crate::keymap::init(cx);
            crate::notify::init(cx);
        });
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.upsert_all(groups).unwrap();
        store.upsert_all(hosts).unwrap();
        let model = cx.update(|cx| HostsModel::install(store, cx));
        cx.run_until_parked();
        let (list, cx) = cx.add_window_view(|window, cx| {
            let list = HostList::new(model.clone(), cx);
            list.list_focus.focus(window);
            list
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let subscription = cx.update(|_, cx| {
            cx.subscribe(&list, move |_, event: &HostListEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        (
            Fixture {
                list,
                model,
                events,
                _subscription: subscription,
            },
            cx,
        )
    }

    fn host(label: &str, address: &str) -> Host {
        let mut h = Host::new(address.parse().unwrap(), at(0));
        h.label = label.into();
        h
    }

    fn titles(list: &Entity<HostList>, cx: &mut VisualTestContext) -> Vec<String> {
        list.read_with(cx, |list, _| {
            list.visible
                .iter()
                .map(|h| h.title().into_owned())
                .collect()
        })
    }

    fn cursor_title(list: &Entity<HostList>, cx: &mut VisualTestContext) -> Option<String> {
        list.read_with(cx, |list, _| {
            let cursor = list.selection.cursor()?;
            list.visible
                .iter()
                .find(|h| h.id == cursor)
                .map(|h| h.title().into_owned())
        })
    }

    #[gpui::test]
    fn keyboard_moves_selects_and_connects(cx: &mut TestAppContext) {
        let mut db = host("db", "10.0.0.5");
        db.settings.username = Some("postgres".into());
        db.settings.port = Port::new(2222);
        let (f, cx) = page(
            &[
                host("web", "web.example.com"),
                db,
                host("app", "app.example.com"),
            ],
            &[],
            cx,
        );
        assert_eq!(titles(&f.list, cx), ["app", "db", "web"]);

        assert_eq!(
            cursor_title(&f.list, cx).as_deref(),
            Some("app"),
            "the cursor starts on the first row"
        );
        cx.simulate_keystrokes("down");
        assert_eq!(cursor_title(&f.list, cx).as_deref(), Some("db"));
        cx.simulate_keystrokes("enter");
        let events = f.events.take();
        let [HostListEvent::Connect(requests)] = events.as_slice() else {
            unreachable!("one connect event, got {events:?}")
        };
        assert_eq!(
            requests
                .iter()
                .map(|r| r.target.to_string())
                .collect::<Vec<_>>(),
            ["postgres@10.0.0.5:2222"]
        );

        // Shift+Down selects a range; Enter connects to all of it, in list order (HOST-13).
        cx.simulate_keystrokes("up shift-down shift-down enter");
        let events = f.events.take();
        let [HostListEvent::Connect(requests)] = events.as_slice() else {
            unreachable!("one connect event, got {events:?}")
        };
        assert_eq!(requests.len(), 3);
        cx.simulate_keystrokes("escape");
        assert_eq!(f.list.read_with(cx, |l, _| l.selection.count()), 0);
    }

    #[gpui::test]
    fn search_filters_as_you_type(cx: &mut TestAppContext) {
        let mut web = host("web", "web.example.com");
        web.tags = Tag::parse_list("prod").unwrap();
        let (f, cx) = page(&[web, host("db", "10.0.0.5")], &[], cx);
        cx.simulate_keystrokes("/");
        cx.simulate_input("prod");
        assert_eq!(titles(&f.list, cx), ["web"]);
        // Down moves into the list; Escape in the search clears it.
        cx.simulate_keystrokes("down");
        assert_eq!(cursor_title(&f.list, cx).as_deref(), Some("web"));
        cx.simulate_keystrokes("/ escape");
        assert_eq!(titles(&f.list, cx), ["db", "web"]);
    }

    #[gpui::test]
    fn favourites_and_trash(cx: &mut TestAppContext) {
        let (f, cx) = page(
            &[host("a", "a.example.com"), host("b", "b.example.com")],
            &[],
            cx,
        );
        cx.simulate_keystrokes("f");
        cx.run_until_parked();
        let favourite = f.model.read_with(cx, |m, _| {
            m.hosts().iter().filter(|h| h.is_favourite).count()
        });
        assert_eq!(favourite, 1);

        cx.simulate_keystrokes("delete");
        cx.run_until_parked();
        assert_eq!(titles(&f.list, cx), ["b"]);
        assert_eq!(
            cursor_title(&f.list, cx).as_deref(),
            Some("b"),
            "the cursor stays on screen"
        );

        f.list.update(cx, |list, cx| list.show(Section::Trash, cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            f.model.read_with(cx, |m, _| m.hosts().len()),
            2,
            "Enter restores in the trash"
        );
    }

    #[gpui::test]
    fn nav_walks_sections_and_groups(cx: &mut TestAppContext) {
        let prod = Group::new("prod", None, at(0));
        let db = Group::new("db", Some(prod.id), at(0));
        let mut inner = host("inner", "inner.example.com");
        inner.group = Some(db.id);
        let (f, cx) = page(
            &[inner, host("loose", "loose.example.com")],
            &[prod.clone(), db.clone()],
            cx,
        );
        cx.update(|window, cx| f.list.read(cx).nav_focus.focus(window));
        // All → Favourites → Recent → prod.
        cx.simulate_keystrokes("down down down");
        assert_eq!(
            f.list.read_with(cx, |l, _| l.section.clone()),
            Section::Hosts(Scope::Group(prod.id))
        );
        assert_eq!(
            titles(&f.list, cx),
            ["inner"],
            "a group includes its subgroups"
        );
        cx.simulate_keystrokes("right down");
        assert_eq!(
            f.list.read_with(cx, |l, _| l.section.clone()),
            Section::Hosts(Scope::Group(db.id))
        );
        // Left on a collapsed group goes to its parent; Left again collapses the parent.
        cx.simulate_keystrokes("left left");
        f.list.read_with(cx, |l, _| {
            assert_eq!(l.section, Section::Hosts(Scope::Group(prod.id)));
            assert!(!l.expanded.contains(&prod.id));
        });
        // Enter moves into the list.
        cx.simulate_keystrokes("enter");
        assert_eq!(cursor_title(&f.list, cx).as_deref(), Some("inner"));
    }

    #[gpui::test]
    fn sort_picker_reorders(cx: &mut TestAppContext) {
        let mut old = host("b-old", "b.example.com");
        old.created_at = at(1);
        let mut new = host("a-new", "a.example.com");
        new.created_at = at(2);
        let mut used = host("c-used", "c.example.com");
        used.last_connected_at = Some(at(5));
        let (f, cx) = page(&[old, new, used], &[], cx);
        assert_eq!(titles(&f.list, cx), ["a-new", "b-old", "c-used"]);
        cx.simulate_keystrokes("s");
        cx.simulate_input("last");
        cx.simulate_keystrokes("enter");
        assert_eq!(titles(&f.list, cx), ["c-used", "a-new", "b-old"]);
        assert_eq!(f.list.read_with(cx, |l, _| l.sort), SortOrder::LastUsed);
    }

    #[gpui::test]
    fn moving_to_a_new_group_creates_it(cx: &mut TestAppContext) {
        let (f, cx) = page(&[host("web", "web.example.com")], &[], cx);
        cx.simulate_keystrokes("m");
        cx.simulate_input("Fresh");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        f.model.read_with(cx, |m, _| {
            let fresh = m
                .groups()
                .iter()
                .find(|g| g.name == "Fresh")
                .expect("created");
            assert_eq!(m.hosts().first().and_then(|h| h.group), Some(fresh.id));
        });
    }

    #[gpui::test]
    fn new_duplicate_move_and_bulk_edit_from_the_keyboard(cx: &mut TestAppContext) {
        let prod = Group::new("prod", None, at(0));
        let (f, cx) = page(
            &[host("web", "web.example.com")],
            std::slice::from_ref(&prod),
            cx,
        );

        // N opens the editor; typing an address and Enter saves, and the cursor lands on it.
        cx.simulate_keystrokes("n");
        assert!(f.list.read_with(cx, |l, _| l.editor.is_some()));
        cx.simulate_input("api.example.com");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(f.list.read_with(cx, |l, _| l.editor.is_none()));
        assert_eq!(titles(&f.list, cx), ["api.example.com", "web"]);
        assert_eq!(
            cursor_title(&f.list, cx).as_deref(),
            Some("api.example.com")
        );

        // Ctrl/Cmd+D duplicates.
        cx.simulate_keystrokes("secondary-d");
        cx.run_until_parked();
        assert_eq!(
            titles(&f.list, cx),
            ["api.example.com", "api.example.com (copy)", "web"]
        );

        // M moves the selection into a group.
        cx.simulate_keystrokes("secondary-a m");
        cx.simulate_input("prod");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let in_prod = f.model.read_with(cx, |m, _| {
            m.hosts()
                .iter()
                .filter(|h| h.group == Some(prod.id))
                .count()
        });
        assert_eq!(in_prod, 3);

        // E with several selected opens one editor for all of them.
        cx.simulate_keystrokes("secondary-a e");
        let bulk = f.list.read_with(cx, |l, cx| {
            l.editor.as_ref().map(|(e, _)| e.read(cx).title())
        });
        assert_eq!(bulk.as_deref(), Some("Edit 3 hosts"));
        cx.simulate_keystrokes("escape");
        assert!(f.list.read_with(cx, |l, _| l.editor.is_none()));
    }
}
