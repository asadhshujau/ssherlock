//! [`IdentityEditor`]: the form for an identity (KEY-02, KEY-03): its name, user name, key and
//! saved password.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Subscription,
    Window, div,
};
use ssherlock_core::keychain::{Identity, IdentityError, Key};
use ssherlock_core::{Id, Timestamp};

use super::actions::{CancelIdentityEditor, SaveIdentityEditor};
use crate::components::{
    Button, Checkbox, Form, FormField, FormSection, Picker, PickerEvent, PickerItem, TextInput,
    sheet,
};
use crate::hosts::HostsModel;
use crate::ssh::status::capitalise;
use crate::theme::ActiveTheme as _;

/// What the form tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityEditorEvent {
    /// Closed: with the identity saved, or `None` if cancelled.
    Done(Option<Id<Identity>>),
}

/// The form's fields, for placing error messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdentityField {
    /// The name.
    Label,
    /// The user name.
    Username,
    /// The password.
    Password,
}

/// The identity form, for a new identity or an existing one.
pub struct IdentityEditor {
    model: Entity<HostsModel>,
    /// The identity being edited; a new one has nothing saved yet.
    identity: Identity,
    is_new: bool,
    label: Entity<TextInput>,
    username: Entity<TextInput>,
    password: Entity<TextInput>,
    key: Option<Id<Key>>,
    forget_password: bool,
    key_focus: FocusHandle,
    forget_focus: FocusHandle,
    picker: Option<(Entity<Picker>, Subscription)>,
    errors: HashMap<IdentityField, SharedString>,
    focus_handle: FocusHandle,
}

impl EventEmitter<IdentityEditorEvent> for IdentityEditor {}

impl Focusable for IdentityEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.picker {
            Some((picker, _)) => picker.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl IdentityEditor {
    /// A form for `identity`, or for a new one when `None`, with its first field focused.
    pub fn new(
        model: Entity<HostsModel>,
        identity: Option<Identity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let is_new = identity.is_none();
        let identity = identity.unwrap_or_else(|| Identity::new("", Timestamp::now()));
        let password_placeholder = if identity.password.is_some() {
            "Saved; type to replace it"
        } else {
            "Optional; asked for when needed if empty"
        };
        let label = identity.label.clone();
        let username = identity.username.clone();
        let editor = Self {
            label: cx.new(|cx| TextInput::field(label, cx).placeholder("e.g. Production deploy")),
            username: cx.new(|cx| TextInput::field(username, cx).placeholder("e.g. deploy")),
            password: cx.new(|cx| {
                TextInput::field("", cx)
                    .secret()
                    .placeholder(password_placeholder)
            }),
            key: identity.key,
            identity,
            is_new,
            model,
            forget_password: false,
            key_focus: cx.focus_handle().tab_stop(true),
            forget_focus: cx.focus_handle().tab_stop(true),
            picker: None,
            errors: HashMap::new(),
            focus_handle: cx.focus_handle(),
        };
        editor.label.focus_handle(cx).focus(window);
        editor
    }

    /// The error shown for `field`, if any.
    pub fn error(&self, field: IdentityField) -> Option<&str> {
        self.errors.get(&field).map(SharedString::as_ref)
    }

    fn fail(&mut self, field: IdentityField, message: impl Into<String>) {
        self.errors
            .entry(field)
            .or_insert_with(|| capitalise(&message.into()).into());
    }

    fn key_label(&self, cx: &App) -> String {
        match self.key {
            None => "No key".to_owned(),
            Some(id) => self
                .model
                .read(cx)
                .key(id)
                .map_or_else(|| "A deleted key".to_owned(), |k| k.title().to_owned()),
        }
    }

    fn open_key_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<(Option<Id<Key>>, String, String)> = std::iter::once((
            None,
            "No key".to_owned(),
            "Log in with the password, the agent or ~/.ssh".to_owned(),
        ))
        .chain(
            super::list::sorted(self.model.read(cx).keys())
                .into_iter()
                .map(|k| (Some(k.id), k.title().to_owned(), k.kind.display_name())),
        )
        .collect();
        let items = keys
            .iter()
            .map(|(_, label, detail)| PickerItem::new(label.clone()).detail(detail.clone()))
            .collect();
        let current = keys
            .iter()
            .position(|(id, ..)| *id == self.key)
            .unwrap_or(0);
        let picker = cx.new(|cx| Picker::new("Key", items, window, cx).with_selected(current));
        let subscription = cx.subscribe_in(&picker, window, move |editor, _, event, window, cx| {
            if let PickerEvent::Picked(index) = event
                && let Some((id, ..)) = keys.get(*index)
            {
                editor.key = *id;
            }
            editor.picker = None;
            editor.key_focus.focus(window);
            cx.notify();
        });
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    fn save(&mut self, _: &SaveIdentityEditor, _: &mut Window, cx: &mut Context<Self>) {
        self.errors.clear();
        let typed = {
            let input = self.password.read(cx);
            (!input.is_empty()).then(|| input.secret_text())
        };
        let password = match typed {
            Some(typed) => {
                let model = self.model.read(cx);
                let sealed = match model.vault() {
                    Some(vault) => vault.seal_str(&typed).map_err(|e| e.to_string()),
                    None => Err(format!(
                        "the password can't be saved: {}",
                        model.vault_problem().unwrap_or_default()
                    )),
                };
                match sealed {
                    Ok(sealed) => Some(sealed),
                    Err(message) => {
                        self.fail(IdentityField::Password, message);
                        cx.notify();
                        return;
                    }
                }
            }
            None if self.forget_password => None,
            None => self.identity.password.clone(),
        };
        let identity = Identity {
            label: self.label.read(cx).text().trim().to_owned(),
            username: self.username.read(cx).text().trim().to_owned(),
            password,
            key: self.key,
            updated_at: Timestamp::now(),
            ..self.identity.clone()
        };
        if let Err(error) = identity.validate() {
            let field = match error {
                IdentityError::LabelTooLong | IdentityError::LabelControl => IdentityField::Label,
                IdentityError::EmptyUsername | IdentityError::InvalidUsername(..) => {
                    IdentityField::Username
                }
            };
            self.fail(field, error.to_string());
            cx.notify();
            return;
        }
        let id = identity.id;
        self.model
            .update(cx, |model, cx| model.save_identity(identity, cx));
        cx.emit(IdentityEditorEvent::Done(Some(id)));
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn cancel(&mut self, _: &CancelIdentityEditor, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(IdentityEditorEvent::Done(None));
    }
}

impl Render for IdentityEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error = |field: IdentityField| self.errors.get(&field).cloned();
        let title = if self.is_new {
            "New identity".to_owned()
        } else {
            format!("Edit {}", self.identity.title())
        };
        let has_saved_password = self.identity.password.is_some();
        let form = Form::new()
            .child(
                FormSection::new("Identity")
                    .child(
                        FormField::new("Name", self.label.clone())
                            .hint("Shown in pickers; the user name if empty")
                            .error(error(IdentityField::Label)),
                    )
                    .child(
                        FormField::new("User name", self.username.clone())
                            .error(error(IdentityField::Username)),
                    ),
            )
            .child(
                FormSection::new("Credentials")
                    .child(
                        FormField::new(
                            "Key",
                            Button::new("identity-key", self.key_label(cx))
                                .dropdown()
                                .focus_handle(&self.key_focus)
                                .on_click(cx.listener(|editor, _, window, cx| {
                                    editor.open_key_picker(window, cx);
                                })),
                        )
                        .hint("Tried first, before the agent and ~/.ssh"),
                    )
                    .child(
                        FormField::new("Password", self.password.clone())
                            .error(error(IdentityField::Password)),
                    )
                    .when(has_saved_password, |section| {
                        section.child(FormField::new(
                            "",
                            Checkbox::new(
                                "forget-password",
                                "Forget the saved password",
                                self.forget_password,
                                &self.forget_focus,
                            )
                            .on_toggle(cx.listener(
                                |editor, value: &bool, _, cx| {
                                    editor.forget_password = *value;
                                    cx.notify();
                                },
                            )),
                        ))
                    }),
            );
        sheet(
            &theme,
            title,
            "Save",
            div().child(form).into_any_element(),
            cx.listener(|editor, _, window, cx| {
                editor.cancel(&CancelIdentityEditor, window, cx);
            }),
            cx.listener(|editor, _, window, cx| editor.save(&SaveIdentityEditor, window, cx)),
        )
        .key_context("IdentityEditor")
        .track_focus(&self.focus_handle)
        .on_action(cx.listener(Self::save))
        .on_action(cx.listener(Self::cancel))
        .when_some(self.picker.as_ref(), |d, (picker, _)| {
            d.child(picker.clone())
        })
    }
}
