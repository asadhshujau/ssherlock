//! [`IdentityList`]: the identities page (KEY-02, KEY-03, KEY-14). Identities in a list, the
//! one under the cursor in detail, and forms to add, edit and delete them.
//!
//! Like [`KeyList`](super::KeyList), it holds view state only and hands its dialogs to the hosts
//! page to draw over the whole page.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::Id;
use ssherlock_core::keychain::{Identity, Usage, usage_of_identity};

use super::actions::{DeleteIdentity, EditIdentity, NewIdentity};
use super::detail::usage_lines;
use super::identity_editor::{IdentityEditor, IdentityEditorEvent};
use crate::components::{Button, ButtonStyle, Modal, ModalEvent};
use crate::hosts::HostsModel;
use crate::hosts::actions::{SelectFirst, SelectLast, SelectNext, SelectPrevious};
use crate::theme::{ActiveTheme as _, Theme};

/// The identities in the order the page lists them: by title, ignoring case.
pub fn sorted(identities: &[Identity]) -> Vec<&Identity> {
    let mut sorted: Vec<&Identity> = identities.iter().collect();
    sorted.sort_by(|a, b| {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then(a.created_at.cmp(&b.created_at))
    });
    sorted
}

/// A dialog open over the page.
enum Dialog {
    Editor(Entity<IdentityEditor>),
    Delete(Entity<Modal>),
}

/// The identities page.
pub struct IdentityList {
    model: Entity<HostsModel>,
    cursor: Option<Id<Identity>>,
    usage: HashMap<Id<Identity>, Usage>,
    dialog: Option<(Dialog, Subscription)>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for IdentityList {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.dialog {
            Some((Dialog::Editor(editor), _)) => editor.focus_handle(cx),
            Some((Dialog::Delete(modal), _)) => modal.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl IdentityList {
    /// The page over `model`.
    pub fn new(model: Entity<HostsModel>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&model, |list, _, cx| list.refresh(cx))];
        let mut list = Self {
            model,
            cursor: None,
            usage: HashMap::new(),
            dialog: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        list.refresh(cx);
        list
    }

    /// The identity under the cursor.
    pub fn cursor(&self) -> Option<Id<Identity>> {
        self.cursor
    }

    /// The form, if it's open; for tests.
    pub fn editor(&self) -> Option<&Entity<IdentityEditor>> {
        match &self.dialog {
            Some((Dialog::Editor(editor), _)) => Some(editor),
            _ => None,
        }
    }

    /// The dialog over the page, for the hosts page to draw above everything.
    pub fn overlay(&self) -> Option<AnyElement> {
        self.dialog.as_ref().map(|(dialog, _)| match dialog {
            Dialog::Editor(editor) => editor.clone().into_any_element(),
            Dialog::Delete(modal) => modal.clone().into_any_element(),
        })
    }

    fn visible_ids(&self, cx: &App) -> Vec<Id<Identity>> {
        sorted(self.model.read(cx).identities())
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        self.usage = model
            .identities()
            .iter()
            .map(|i| {
                (
                    i.id,
                    usage_of_identity(i.id, model.hosts(), model.groups(), model.identities()),
                )
            })
            .collect();
        let ids = self.visible_ids(cx);
        if self.cursor.is_none_or(|id| !ids.contains(&id)) {
            self.cursor = ids.first().copied();
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.visible_ids(cx);
        let Some(last) = ids.len().checked_sub(1) else {
            return;
        };
        let at = self
            .cursor
            .and_then(|id| ids.iter().position(|i| *i == id))
            .unwrap_or(0);
        let next = at.saturating_add_signed(delta).min(last);
        self.cursor = ids.get(next).copied();
        self.scroll.scroll_to_item(next);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.step(isize::MIN, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.step(isize::MAX, cx);
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn open_editor(
        &mut self,
        identity: Option<Identity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let editor = cx.new(|cx| IdentityEditor::new(model, identity, window, cx));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |list, _, event: &IdentityEditorEvent, window, cx| {
                let IdentityEditorEvent::Done(saved) = event;
                if let Some(id) = saved {
                    list.cursor = Some(*id);
                }
                list.close_dialog(window, cx);
            },
        );
        self.dialog = Some((Dialog::Editor(editor), subscription));
        cx.notify();
    }

    fn new_identity(&mut self, _: &NewIdentity, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(None, window, cx);
    }

    fn cursor_identity(&self, cx: &App) -> Option<Identity> {
        self.cursor
            .and_then(|id| self.model.read(cx).identity(id))
            .cloned()
    }

    fn edit(&mut self, _: &EditIdentity, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(identity) = self.cursor_identity(cx) {
            self.open_editor(Some(identity), window, cx);
        }
    }

    fn delete(&mut self, _: &DeleteIdentity, window: &mut Window, cx: &mut Context<Self>) {
        let Some(identity) = self.cursor_identity(cx) else {
            return;
        };
        let used = self.usage.get(&identity.id).map_or(0, |u| u.hosts.len());
        let message = match used {
            0 => "Its saved password goes with it. This can't be undone.".to_owned(),
            1 => "1 host uses it and will no longer. This can't be undone.".to_owned(),
            n => format!("{n} hosts use it and will no longer. This can't be undone."),
        };
        let modal = cx.new(|cx| {
            Modal::new(
                format!("Delete {}?", identity.title()),
                message,
                "Delete",
                window,
                cx,
            )
            .danger()
        });
        let id = identity.id;
        let subscription = cx.subscribe_in(
            &modal,
            window,
            move |list, _, event: &ModalEvent, window, cx| {
                list.close_dialog(window, cx);
                if *event == ModalEvent::Confirmed {
                    list.model
                        .update(cx, |model, cx| model.delete_identity(id, cx));
                }
            },
        );
        self.dialog = Some((Dialog::Delete(modal), subscription));
        cx.notify();
    }

    /// The delete confirmation, if it's open; for tests.
    pub fn delete_dialog(&self) -> Option<Entity<Modal>> {
        match &self.dialog {
            Some((Dialog::Delete(modal), _)) => Some(modal.clone()),
            _ => None,
        }
    }
}

/// One line about an identity's credentials: `deploy · key Laptop · password saved`.
pub fn credentials_line(identity: &Identity, key_title: Option<&str>) -> String {
    let mut parts = vec![identity.username.clone()];
    if let Some(key) = key_title {
        parts.push(format!("key {key}"));
    }
    if identity.password.is_some() {
        parts.push("password saved".to_owned());
    }
    parts.join(" \u{00b7} ")
}

impl Render for IdentityList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let is_focused = self.focus_handle.contains_focused(window, cx);
        let model = self.model.read(cx);
        let identities: Vec<Identity> = sorted(model.identities()).into_iter().cloned().collect();
        let key_title = |identity: &Identity| {
            identity
                .key
                .and_then(|id| model.key(id))
                .map(|k| k.title().to_owned())
        };
        let lines: Vec<String> = identities
            .iter()
            .map(|i| credentials_line(i, key_title(i).as_deref()))
            .collect();

        let toolbar = div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap(theme.spacing.md)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Identities"))
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(identities.len().to_string()),
                    ),
            )
            .child(
                Button::new("new-identity", "New identity")
                    .style(ButtonStyle::Primary)
                    .on_click(cx.listener(|list, _, window, cx| {
                        list.new_identity(&NewIdentity, window, cx);
                    })),
            );

        let rows = identities
            .iter()
            .zip(lines)
            .enumerate()
            .map(|(index, (identity, line))| {
                let is_cursor = self.cursor == Some(identity.id);
                let hosts = self.usage.get(&identity.id).map_or(0, |u| u.hosts.len());
                let id = identity.id;
                let hover = theme.colors.control_hover;
                div()
                    .id(("identity", index))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.md)
                    .px(theme.spacing.md)
                    .py(theme.spacing.sm)
                    .rounded(theme.radius.sm)
                    .cursor_pointer()
                    .when(is_cursor, |d| {
                        d.bg(if is_focused {
                            theme.colors.row_selected
                        } else {
                            theme.colors.control_hover
                        })
                    })
                    .when(!is_cursor, |d| d.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |list, event: &ClickEvent, window, cx| {
                        list.cursor = Some(id);
                        list.focus_handle.focus(window);
                        if event.click_count() == 2 {
                            list.edit(&EditIdentity, window, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(identity.title().to_owned()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(theme.text.small_size)
                                    .text_color(theme.colors.text_muted)
                                    .child(line),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(match hosts {
                                0 => String::new(),
                                1 => "1 host".to_owned(),
                                n => format!("{n} hosts"),
                            }),
                    )
            });

        let list = div()
            .id("identity-list")
            .key_context("IdentityList")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::new_identity))
            .on_action(cx.listener(Self::edit))
            .on_action(cx.listener(Self::delete))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap(theme.spacing.xs)
            .when(identities.is_empty(), |d| d.child(empty_state(&theme)))
            .children(rows);

        let panel = self.cursor_identity(cx).map(|identity| {
            let model = self.model.read(cx);
            let usage = self.usage.get(&identity.id).cloned().unwrap_or_default();
            let facts = super::detail::KeyFacts {
                fingerprints: None,
                hosts: usage
                    .hosts
                    .iter()
                    .filter_map(|id| model.host(*id).map(|h| h.title().into_owned()))
                    .collect(),
                identities: Vec::new(),
                groups: usage
                    .groups
                    .iter()
                    .filter_map(|id| {
                        model
                            .groups()
                            .iter()
                            .find(|g| g.id == *id)
                            .map(|g| g.name.clone())
                    })
                    .collect(),
            };
            let key = key_title(&identity).unwrap_or_else(|| "None".to_owned());
            identity_panel(
                &identity,
                &key,
                &usage_lines(&facts),
                &theme,
                cx.listener(|list, _, window, cx| list.edit(&EditIdentity, window, cx)),
                cx.listener(|list, _, window, cx| list.delete(&DeleteIdentity, window, cx)),
            )
        });

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.spacing.lg)
                    .p(theme.spacing.xl)
                    .child(toolbar)
                    .child(list),
            )
            .children(panel)
    }
}

fn identity_panel(
    identity: &Identity,
    key: &str,
    usage: &[String],
    theme: &Theme,
    on_edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_delete: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let row = |title: &'static str, value: SharedString| {
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .child(
                div()
                    .text_size(theme.text.small_size)
                    .text_color(theme.colors.text_muted)
                    .child(title),
            )
            .child(div().child(value))
    };
    let password = if identity.password.is_some() {
        "Saved in the vault"
    } else {
        "Asked for when needed"
    };
    div()
        .id("identity-panel")
        .flex_none()
        .w(theme.metrics.detail_width)
        .h_full()
        .overflow_y_scroll()
        .border_l(theme.metrics.border_width)
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .text_size(theme.text.ui_size)
        .text_color(theme.colors.text)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(theme.spacing.lg)
                .p(theme.spacing.xl)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(identity.title().to_owned()),
                )
                .child(row("User name", identity.username.clone().into()))
                .child(row("Key", key.to_owned().into()))
                .child(row("Password", password.into()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.xs)
                        .child(
                            div()
                                .text_size(theme.text.small_size)
                                .text_color(theme.colors.text_muted)
                                .child("Used by"),
                        )
                        .children(usage.iter().map(|line| div().child(line.clone()))),
                )
                .child(
                    div()
                        .flex()
                        .gap(theme.spacing.md)
                        .child(Button::new("identity-edit", "Edit").on_click(on_edit))
                        .child(
                            Button::new("identity-delete", "Delete")
                                .style(ButtonStyle::Danger)
                                .on_click(on_delete),
                        ),
                ),
        )
}

fn empty_state(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.sm)
        .py(theme.spacing.xl)
        .text_color(theme.colors.text_muted)
        .child("No identities yet.")
        .child(
            "An identity is a user name with a password or key. Pick it for many hosts, and \
             change it once for all of them.",
        )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use ssherlock_core::{Sealed, Timestamp};

    use super::*;

    #[test]
    fn credentials_read_naturally() {
        let mut identity = Identity::new("deploy", Timestamp::UNIX_EPOCH);
        assert_eq!(credentials_line(&identity, None), "deploy");
        identity.password = Some(Sealed::from_bytes(vec![1]));
        assert_eq!(
            credentials_line(&identity, Some("Laptop")),
            "deploy \u{00b7} key Laptop \u{00b7} password saved"
        );
    }
}
