//! A searchable list to pick one item from with the keyboard: type to filter, Up/Down to move,
//! Enter to pick, Escape to close. Used to choose a group, to move hosts, and by any feature
//! that needs "pick one of these".

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::fuzzy;

use super::context_menu::{ConfirmItem, Dismiss, SelectNext, SelectPrevious};
use super::text_input::{TextInput, TextInputEvent};
use crate::theme::ActiveTheme as _;

/// One choice in a [`Picker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    /// What's matched against the typed text and shown.
    pub label: SharedString,
    /// Shown faded after the label.
    pub detail: Option<SharedString>,
}

impl PickerItem {
    /// An item showing `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
        }
    }

    /// Adds faded text after the label.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// What a [`Picker`] reports. Its owner drops it after either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerEvent {
    /// The item at this index of the list passed to [`Picker::new`] was chosen.
    Picked(usize),
    /// "Create …" was chosen for this text (see [`Picker::creating`]).
    Create(String),
    /// Closed without choosing.
    Dismissed,
}

/// A floating, focused list with a filter field.
pub struct Picker {
    items: Vec<PickerItem>,
    /// Indices into `items` that match the filter, best first.
    matches: Vec<usize>,
    /// Index into the rows: `matches`, plus the create row if shown.
    selected: usize,
    /// What a new item is called ("group"), if typing a new name offers to create one.
    create_noun: Option<SharedString>,
    /// The filter text, trimmed.
    query: String,
    input: Entity<TextInput>,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Picker {
    /// A picker over `items`, with `placeholder` in its empty filter field, taking focus.
    pub fn new(
        placeholder: impl Into<SharedString>,
        items: Vec<PickerItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = placeholder.into();
        let input = cx.new(|cx| TextInput::field("", cx).placeholder(placeholder));
        input.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&input, |picker, _, event: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(query) = event {
                picker.filter(query);
                cx.notify();
            }
        });
        let matches = (0..items.len()).collect();
        Self {
            items,
            matches,
            selected: 0,
            create_noun: None,
            query: String::new(),
            input,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    /// Selects the item at `index` of the original list, e.g. the current value.
    #[must_use]
    pub fn with_selected(mut self, index: usize) -> Self {
        if let Some(position) = self.matches.iter().position(|&m| m == index) {
            self.selected = position;
            self.scroll.scroll_to_item(position);
        }
        self
    }

    /// Offers to create a new item called `noun` when the typed text isn't an existing item's
    /// label: a "Create group “…”" row, first when nothing matches and last otherwise.
    #[must_use]
    pub fn creating(mut self, noun: impl Into<SharedString>) -> Self {
        self.create_noun = Some(noun.into());
        self
    }

    /// Whether the create row shows, and where: after the matches, or alone.
    fn create_row(&self) -> Option<usize> {
        self.create_noun.as_ref()?;
        if self.query.is_empty() {
            return None;
        }
        let exists = self
            .items
            .iter()
            .any(|item| item.label.eq_ignore_ascii_case(&self.query));
        (!exists).then_some(self.matches.len())
    }

    fn row_count(&self) -> usize {
        self.matches.len() + usize::from(self.create_row().is_some())
    }

    fn filter(&mut self, query: &str) {
        query.trim().clone_into(&mut self.query);
        let indices: Vec<usize> = (0..self.items.len()).collect();
        self.matches = fuzzy::filter(indices, query, |&index| {
            self.items
                .get(index)
                .map(|item| item.label.to_string())
                .unwrap_or_default()
        });
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        self.selected = if forward {
            (self.selected + 1) % count
        } else {
            self.selected.checked_sub(1).unwrap_or(count - 1)
        };
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn pick(&mut self, position: usize, cx: &mut Context<Self>) {
        if let Some(&index) = self.matches.get(position) {
            cx.emit(PickerEvent::Picked(index));
        } else if self.create_row() == Some(position) {
            cx.emit(PickerEvent::Create(self.query.clone()));
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    fn confirm(&mut self, _: &ConfirmItem, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PickerEvent::Dismissed);
    }

    /// The labels currently listed, best match first; for tests.
    #[cfg(test)]
    fn visible(&self) -> Vec<String> {
        self.matches
            .iter()
            .filter_map(|&i| self.items.get(i).map(|item| item.label.to_string()))
            .collect()
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self
            .matches
            .iter()
            .enumerate()
            .filter_map(|(position, &index)| {
                let item = self.items.get(index)?;
                let is_selected = position == self.selected;
                let hover = theme.colors.control_hover;
                Some(
                    div()
                        .id(position)
                        .flex()
                        .items_center()
                        .gap(theme.spacing.md)
                        .px(theme.spacing.md)
                        .py(theme.spacing.sm)
                        .rounded(theme.radius.sm)
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(theme.colors.control_hover))
                        .when(!is_selected, |d| d.hover(move |s| s.bg(hover)))
                        .on_click(cx.listener(move |picker, _, _, cx| picker.pick(position, cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(item.label.clone()),
                        )
                        .when_some(item.detail.clone(), |d, detail| {
                            d.child(div().text_color(theme.colors.text_muted).child(detail))
                        }),
                )
            });
        let create = self
            .create_row()
            .zip(self.create_noun.clone())
            .map(|(position, noun)| {
                let is_selected = position == self.selected;
                let hover = theme.colors.control_hover;
                div()
                    .id("picker-create")
                    .px(theme.spacing.md)
                    .py(theme.spacing.sm)
                    .rounded(theme.radius.sm)
                    .cursor_pointer()
                    .text_color(theme.colors.accent)
                    .when(is_selected, |d| d.bg(theme.colors.control_hover))
                    .when(!is_selected, |d| d.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |picker, _, _, cx| picker.pick(position, cx)))
                    .child(format!("Create {noun} \u{201c}{}\u{201d}", self.query))
            });
        let is_empty = self.matches.is_empty() && create.is_none();
        div()
            .key_context("Picker")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .absolute()
            .top(theme.spacing.xl)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("picker")
                    // It floats over other views, which would otherwise get the click too.
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
                    .child(self.input.clone())
                    .child(
                        div()
                            .id("picker-list")
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .max_h(theme.metrics.picker_max_height)
                            .flex()
                            .flex_col()
                            .children(rows)
                            .children(create)
                            .when(is_empty, |d| {
                                d.child(
                                    div()
                                        .px(theme.spacing.md)
                                        .py(theme.spacing.sm)
                                        .text_color(theme.colors.text_muted)
                                        .child("No matches"),
                                )
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

    fn items() -> Vec<PickerItem> {
        ["Production", "Staging", "prod / db", "Home lab"]
            .into_iter()
            .map(PickerItem::new)
            .collect()
    }

    fn run(keys: &str, typed: &str, cx: &mut TestAppContext) -> (Vec<PickerEvent>, Vec<String>) {
        run_with(keys, typed, |picker| picker, cx)
    }

    fn run_with(
        keys: &str,
        typed: &str,
        setup: impl FnOnce(Picker) -> Picker,
        cx: &mut TestAppContext,
    ) -> (Vec<PickerEvent>, Vec<String>) {
        cx.update(crate::keymap::init);
        let (picker, cx) =
            cx.add_window_view(|window, cx| setup(Picker::new("Group", items(), window, cx)));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&picker, move |_, event: &PickerEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        if !typed.is_empty() {
            cx.simulate_input(typed);
        }
        if !keys.is_empty() {
            cx.simulate_keystrokes(keys);
        }
        cx.run_until_parked();
        let visible = picker.read_with(cx, |picker, _| picker.visible());
        (seen.take(), visible)
    }

    #[gpui::test]
    fn enter_picks_the_first_item(cx: &mut TestAppContext) {
        let (events, visible) = run("enter", "", cx);
        assert_eq!(events, [PickerEvent::Picked(0)]);
        assert_eq!(visible.len(), 4);
    }

    #[gpui::test]
    fn typing_filters_and_arrows_move(cx: &mut TestAppContext) {
        let (events, visible) = run("down enter", "prod", cx);
        assert_eq!(visible, ["Production", "prod / db"]);
        // The second match is "prod / db", index 2 of the original list.
        assert_eq!(events, [PickerEvent::Picked(2)]);
    }

    #[gpui::test]
    fn up_wraps_to_the_last_match(cx: &mut TestAppContext) {
        let (events, _) = run("up enter", "", cx);
        assert_eq!(events, [PickerEvent::Picked(3)]);
    }

    #[gpui::test]
    fn escape_dismisses(cx: &mut TestAppContext) {
        let (events, _) = run("escape", "", cx);
        assert_eq!(events, [PickerEvent::Dismissed]);
    }

    #[gpui::test]
    fn a_new_name_can_be_created(cx: &mut TestAppContext) {
        let (events, _) = run_with("enter", " Databases ", |p| p.creating("group"), cx);
        assert_eq!(events, [PickerEvent::Create("Databases".into())]);
    }

    #[gpui::test]
    fn create_comes_after_the_matches(cx: &mut TestAppContext) {
        // "prod" matches two groups; Enter still picks the first, Up wraps to "Create".
        let (events, _) = run_with("enter", "prod", |p| p.creating("group"), cx);
        assert_eq!(events, [PickerEvent::Picked(0)]);
    }

    #[gpui::test]
    fn up_from_the_first_match_reaches_create(cx: &mut TestAppContext) {
        let (events, _) = run_with("up enter", "prod", |p| p.creating("group"), cx);
        assert_eq!(events, [PickerEvent::Create("prod".into())]);
    }

    #[gpui::test]
    fn an_existing_name_is_not_offered_again(cx: &mut TestAppContext) {
        let (events, _) = run_with("up enter", "staging", |p| p.creating("group"), cx);
        assert_eq!(events, [PickerEvent::Picked(1)]);
    }

    #[gpui::test]
    fn no_match_picks_nothing(cx: &mut TestAppContext) {
        let (events, visible) = run("enter", "zzz", cx);
        assert_eq!(events, []);
        assert_eq!(visible, Vec::<String>::new());
    }
}
