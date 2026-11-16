//! [`HostEditor`]: the form for a host, a group, or many hosts at once (HOST-01, 02, 08,
//! CONN-17, CONN-18).
//!
//! Built only from the shared form components. Fields left empty inherit (or, in a bulk edit,
//! keep each host's own value); the placeholders say what would be inherited and from where.
//! Saving checks everything first and shows each problem under its field; nothing is saved
//! until all of them are fixed.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Subscription,
    Window,
};
use ssherlock_core::hosts::{
    BulkEdit, Change, Colour, EffectiveSettings, Group, GroupError, GroupTree, Host, HostError,
    HostSettings, Resolved, SettingsError, Source, Tag,
};
use ssherlock_core::keychain::{Identity, Key};
use ssherlock_core::{Address, AlgorithmPreferences, Compression, Id, Sealed, Timestamp};
use ssherlock_ssh::SshError;

use super::actions::{CancelEditor, SaveEditor};
use super::fields;
use super::model::HostsModel;
use crate::components::{
    Button, Checkbox, Form, FormField, FormSection, Picker, PickerEvent, PickerItem, Segment,
    SegmentedControl, TextInput, sheet,
};
use crate::ssh::SshEnvironment;
use crate::ssh::status::capitalise;
use crate::theme::ActiveTheme as _;

/// What the editor edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorMode {
    /// A new host, starting in this group.
    NewHost {
        /// The group it starts in.
        group: Option<Id<Group>>,
    },
    /// An existing host.
    EditHost(Host),
    /// A new group inside `parent`.
    NewGroup {
        /// The group it goes in.
        parent: Option<Id<Group>>,
    },
    /// An existing group.
    EditGroup(Group),
    /// Several hosts at once.
    Bulk(Vec<Id<Host>>),
}

/// What was saved, so the list can put its cursor there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saved {
    /// A host.
    Host(Id<Host>),
    /// A group.
    Group(Id<Group>),
    /// Several hosts.
    Hosts(Vec<Id<Host>>),
}

/// What the editor reports. Its owner drops it after this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEditorEvent {
    /// Closed: saved (and what), or cancelled (`None`).
    Done(Option<Saved>),
}

/// The fields that can show an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Field {
    Name,
    Address,
    Group,
    Tags,
    RemoveTags,
    Port,
    Username,
    Kex,
    HostKey,
    Cipher,
    Mac,
    Password,
}

/// The text fields. Some are unused in some modes.
struct Inputs {
    /// Host label or group name.
    name: Entity<TextInput>,
    address: Entity<TextInput>,
    /// Tags, or in a bulk edit, tags to add.
    tags: Entity<TextInput>,
    remove_tags: Entity<TextInput>,
    notes: Entity<TextInput>,
    port: Entity<TextInput>,
    username: Entity<TextInput>,
    kex: Entity<TextInput>,
    host_key: Entity<TextInput>,
    cipher: Entity<TextInput>,
    mac: Entity<TextInput>,
    /// A new password to save; empty keeps the saved one (or none).
    password: Entity<TextInput>,
}

/// One row of a credential picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Credential {
    Inherit,
    Identity(Id<Identity>),
    Key(Id<Key>),
}

/// Which credential picker is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialChoice {
    Identity,
    Key,
}

/// The credential settings a host or group picks.
#[derive(Clone, Debug, Default)]
struct Credentials {
    identity: Option<Id<Identity>>,
    key: Option<Id<Key>>,
    password: Option<Sealed>,
}

/// The form for a host, a group, or many hosts.
pub struct HostEditor {
    model: Entity<HostsModel>,
    mode: EditorMode,
    inputs: Inputs,
    /// The host's group, the group's parent, or a bulk move.
    group: Change<Option<Id<Group>>>,
    colour: Change<Option<Colour>>,
    compression: Option<Compression>,
    /// The identity, key and saved password the host or group picks itself.
    credentials: Credentials,
    forget_password: bool,
    identity_focus: FocusHandle,
    key_focus: FocusHandle,
    forget_focus: FocusHandle,
    is_favourite: Change<bool>,
    errors: HashMap<Field, SharedString>,
    group_focus: FocusHandle,
    favourite_focus: FocusHandle,
    colour_focus: FocusHandle,
    compression_focus: FocusHandle,
    picker: Option<(Entity<Picker>, Subscription)>,
    /// Groups created from the group picker, until the model has reloaded with them.
    created: Vec<Group>,
    focus_handle: FocusHandle,
}

impl EventEmitter<HostEditorEvent> for HostEditor {}

impl Focusable for HostEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.picker {
            Some((picker, _)) => picker.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl HostEditor {
    /// An editor for `mode`, with its first field focused.
    pub fn new(
        model: Entity<HostsModel>,
        mode: EditorMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let empty = HostSettings::default();
        let (name, address, tags, notes, settings) = match &mode {
            EditorMode::EditHost(host) => (
                host.label.clone(),
                host.address.to_string(),
                Tag::format_list(&host.tags),
                host.notes.clone(),
                &host.settings,
            ),
            EditorMode::EditGroup(group) => (
                group.name.clone(),
                String::new(),
                String::new(),
                String::new(),
                &group.settings,
            ),
            _ => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                &empty,
            ),
        };
        let field = |text: String, cx: &mut Context<Self>| cx.new(|cx| TextInput::field(text, cx));
        let algorithms = &settings.algorithms;
        let inputs = Inputs {
            name: field(name, cx),
            address: field(address, cx),
            tags: field(tags, cx),
            remove_tags: field(String::new(), cx),
            notes: cx.new(|cx| TextInput::field(notes, cx).multiline()),
            port: field(settings.port.map(|p| p.to_string()).unwrap_or_default(), cx),
            username: field(settings.username.clone().unwrap_or_default(), cx),
            kex: field(fields::format_algorithms(&algorithms.kex), cx),
            host_key: field(fields::format_algorithms(&algorithms.host_key), cx),
            cipher: field(fields::format_algorithms(&algorithms.cipher), cx),
            mac: field(fields::format_algorithms(&algorithms.mac), cx),
            password: cx.new(|cx| TextInput::field("", cx).secret()),
        };
        let (group, colour, is_favourite) = match &mode {
            EditorMode::NewHost { group } => {
                (Change::Set(*group), Change::Set(None), Change::Set(false))
            }
            EditorMode::EditHost(host) => (
                Change::Set(host.group),
                Change::Set(host.settings.colour),
                Change::Set(host.is_favourite),
            ),
            EditorMode::NewGroup { parent } => {
                (Change::Set(*parent), Change::Set(None), Change::Keep)
            }
            EditorMode::EditGroup(group) => (
                Change::Set(group.parent),
                Change::Set(group.settings.colour),
                Change::Keep,
            ),
            EditorMode::Bulk(_) => (Change::Keep, Change::Keep, Change::Keep),
        };
        let first = match &mode {
            EditorMode::NewHost { .. } | EditorMode::EditHost(_) => &inputs.address,
            EditorMode::NewGroup { .. } | EditorMode::EditGroup(_) => &inputs.name,
            EditorMode::Bulk(_) => &inputs.tags,
        };
        first.focus_handle(cx).focus(window);
        let credentials = Credentials {
            identity: settings.identity,
            key: settings.key,
            password: settings.password.clone(),
        };
        let mut editor = Self {
            model,
            compression: settings.compression,
            credentials,
            forget_password: false,
            identity_focus: cx.focus_handle().tab_stop(true),
            key_focus: cx.focus_handle().tab_stop(true),
            forget_focus: cx.focus_handle().tab_stop(true),
            mode,
            inputs,
            group,
            colour,
            is_favourite,
            errors: HashMap::new(),
            group_focus: cx.focus_handle().tab_stop(true),
            favourite_focus: cx.focus_handle().tab_stop(true),
            colour_focus: cx.focus_handle().tab_stop(true),
            compression_focus: cx.focus_handle().tab_stop(true),
            picker: None,
            created: Vec::new(),
            focus_handle: cx.focus_handle(),
        };
        editor.update_placeholders(cx);
        editor
    }

    fn is_bulk(&self) -> bool {
        matches!(self.mode, EditorMode::Bulk(_))
    }

    fn is_group(&self) -> bool {
        matches!(
            self.mode,
            EditorMode::NewGroup { .. } | EditorMode::EditGroup(_)
        )
    }

    fn editing_group_id(&self) -> Option<Id<Group>> {
        match &self.mode {
            EditorMode::EditGroup(group) => Some(group.id),
            _ => None,
        }
    }

    /// What the host (or group) would inherit from the group currently chosen.
    fn inherited(&self, cx: &App) -> Option<EffectiveSettings> {
        let Change::Set(group) = self.group else {
            return None;
        };
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        Some(EffectiveSettings::inherited(
            &tree.ancestors(group),
            model.identities(),
        ))
    }

    /// Puts what would be inherited into the empty fields' placeholders.
    fn update_placeholders(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        let from = |source: Source| match source {
            Source::Group(id) => tree
                .get(id)
                .map_or_else(|| "inherited".to_owned(), |g| format!("from {}", g.name)),
            Source::Own | Source::Default => "default".to_owned(),
        };
        let placeholders: Vec<(Entity<TextInput>, Option<String>)> = match self.inherited(cx) {
            None => {
                let keep = Some("Keep each host's own".to_owned());
                vec![
                    (self.inputs.port.clone(), keep.clone()),
                    (self.inputs.username.clone(), keep),
                    (
                        self.inputs.tags.clone(),
                        Some("Tags to add, e.g. prod, db".to_owned()),
                    ),
                    (
                        self.inputs.remove_tags.clone(),
                        Some("Tags to remove".to_owned()),
                    ),
                ]
            }
            Some(inherited) => {
                let local = cx
                    .try_global::<SshEnvironment>()
                    .and_then(|env| env.local_user.clone());
                let own_identity = self.credentials.identity.and_then(|id| model.identity(id));
                let user = match (&inherited.username, own_identity) {
                    (_, Some(identity)) => {
                        format!("{} (from identity {})", identity.username, identity.title())
                    }
                    (
                        Resolved {
                            value: Some(user),
                            source,
                        },
                        None,
                    ) => format!("{user} ({})", from(*source)),
                    (Resolved { value: None, .. }, None) => match local {
                        Some(local) => format!("{local} (your user name)"),
                        None => "Your user name".to_owned(),
                    },
                };
                let password = if self.credentials.password.is_some() {
                    "Saved; type to replace it".to_owned()
                } else if let Some(identity) = own_identity.filter(|i| i.password.is_some()) {
                    format!("Saved in identity {}", identity.title())
                } else if inherited.password.value.is_some() {
                    format!(
                        "Inherits a saved password ({})",
                        from(inherited.password.source)
                    )
                } else {
                    "Asked for when needed".to_owned()
                };
                let list = |resolved: &Resolved<Vec<String>>| {
                    if resolved.value.is_empty() {
                        "SSHerlock's defaults".to_owned()
                    } else {
                        format!(
                            "{} ({})",
                            fields::format_algorithms(&resolved.value),
                            from(resolved.source)
                        )
                    }
                };
                vec![
                    (
                        self.inputs.address.clone(),
                        Some("example.com, 192.0.2.10 or 2001:db8::1".to_owned()),
                    ),
                    (
                        self.inputs.name.clone(),
                        Some(
                            if self.is_group() {
                                "Production"
                            } else {
                                "Shown in the list; the address if empty"
                            }
                            .to_owned(),
                        ),
                    ),
                    (self.inputs.tags.clone(), Some("prod, db".to_owned())),
                    (
                        self.inputs.port.clone(),
                        Some(format!(
                            "{} ({})",
                            inherited.port.value,
                            from(inherited.port.source)
                        )),
                    ),
                    (self.inputs.username.clone(), Some(user)),
                    (self.inputs.kex.clone(), Some(list(&inherited.kex))),
                    (
                        self.inputs.host_key.clone(),
                        Some(list(&inherited.host_key)),
                    ),
                    (self.inputs.cipher.clone(), Some(list(&inherited.cipher))),
                    (self.inputs.mac.clone(), Some(list(&inherited.mac))),
                    (self.inputs.password.clone(), Some(password)),
                ]
            }
        };
        for (input, text) in placeholders {
            input.update(cx, |input, cx| {
                input.set_placeholder(text.map(Into::into), cx);
            });
        }
    }

    // ── group picker ───────────────────────────────────────────────────────────────────

    /// The choices the group field offers, with their labels.
    /// The saved groups, plus any created here that the model hasn't loaded yet.
    fn known_groups(&self, cx: &App) -> Vec<Group> {
        let mut groups = self.model.read(cx).groups().to_vec();
        for group in &self.created {
            if !groups.iter().any(|g| g.id == group.id) {
                groups.push(group.clone());
            }
        }
        groups
    }

    fn group_choices(&self, cx: &App) -> Vec<(Change<Option<Id<Group>>>, String)> {
        let groups = self.known_groups(cx);
        let tree = GroupTree::new(&groups);
        // A group can't go inside itself or anything inside it.
        let excluded = self
            .editing_group_id()
            .map(|id| tree.descendants(id))
            .unwrap_or_default();
        let mut choices = Vec::new();
        if self.is_bulk() {
            choices.push((Change::Keep, "Keep current groups".to_owned()));
        }
        choices.push((
            Change::Set(None),
            if self.is_group() {
                "Top level"
            } else {
                "No group"
            }
            .to_owned(),
        ));
        choices.extend(
            tree.rows(|_| true)
                .into_iter()
                .filter(|row| !excluded.contains(&row.group.id))
                .map(|row| (Change::Set(Some(row.group.id)), tree.path(row.group.id))),
        );
        choices
    }

    fn group_label(&self, cx: &App) -> String {
        self.group_choices(cx)
            .into_iter()
            .find(|(choice, _)| *choice == self.group)
            .map_or_else(|| "No group".to_owned(), |(_, label)| label)
    }

    fn open_group_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices = self.group_choices(cx);
        let items = choices
            .iter()
            .map(|(_, label)| PickerItem::new(label.clone()))
            .collect();
        let current = choices
            .iter()
            .position(|(c, _)| *c == self.group)
            .unwrap_or(0);
        let picker = cx.new(|cx| {
            Picker::new("Group, or a new group's name", items, window, cx)
                .with_selected(current)
                .creating("group")
        });
        let subscription = cx.subscribe_in(&picker, window, move |editor, _, event, window, cx| {
            match event {
                PickerEvent::Picked(index) => {
                    if let Some((choice, _)) = choices.get(*index) {
                        editor.group = choice.clone();
                        editor.errors.remove(&Field::Group);
                        editor.update_placeholders(cx);
                    }
                }
                PickerEvent::Create(name) => editor.create_group(name, cx),
                PickerEvent::Dismissed => {}
            }
            editor.picker = None;
            editor.group_focus.focus(window);
            cx.notify();
        });
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    /// Creates a top-level group called `name` and chooses it. It's saved straight away, ahead
    /// of the host, so the host can be saved into it even if the editor is then cancelled.
    fn create_group(&mut self, name: &str, cx: &mut Context<Self>) {
        self.errors.remove(&Field::Group);
        let group = Group::new(name, None, Timestamp::now());
        if let Err(error) = group.validate() {
            self.fail(Field::Group, error.to_string());
            return;
        }
        self.group = Change::Set(Some(group.id));
        self.created.push(group.clone());
        self.model
            .update(cx, |model, cx| model.save_group(group, cx));
        self.update_placeholders(cx);
    }

    // ── saving ─────────────────────────────────────────────────────────────────────────

    fn text(input: &Entity<TextInput>, cx: &App) -> String {
        input.read(cx).text().to_owned()
    }

    fn fail(&mut self, field: Field, message: impl Into<String>) {
        self.errors
            .entry(field)
            .or_insert_with(|| capitalise(&message.into()).into());
    }

    /// The connection settings from the fields, noting any problems.
    fn read_settings(&mut self, cx: &App) -> HostSettings {
        let port = match fields::port(&Self::text(&self.inputs.port, cx)) {
            Ok(port) => port,
            Err(message) => {
                self.fail(Field::Port, message);
                None
            }
        };
        let colour = match self.colour {
            Change::Set(colour) => colour,
            Change::Keep => None,
        };
        HostSettings {
            port,
            username: fields::username(&Self::text(&self.inputs.username, cx)),
            colour,
            algorithms: AlgorithmPreferences {
                kex: fields::algorithms(&Self::text(&self.inputs.kex, cx)),
                host_key: fields::algorithms(&Self::text(&self.inputs.host_key, cx)),
                cipher: fields::algorithms(&Self::text(&self.inputs.cipher, cx)),
                mac: fields::algorithms(&Self::text(&self.inputs.mac, cx)),
            },
            compression: self.compression,
            identity: self.credentials.identity,
            key: self.credentials.key,
            password: self.read_password(cx),
        }
    }

    /// The password to save: a typed one sealed by the vault, none if the saved one is to be
    /// forgotten, else the one saved already.
    fn read_password(&mut self, cx: &App) -> Option<Sealed> {
        let typed = {
            let input = self.inputs.password.read(cx);
            (!input.is_empty()).then(|| input.secret_text())
        };
        let Some(typed) = typed else {
            return if self.forget_password {
                None
            } else {
                self.credentials.password.clone()
            };
        };
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
                self.fail(Field::Password, message);
                self.credentials.password.clone()
            }
        }
    }

    /// The label of the identity or key button: the choice, or "Inherit".
    fn credential_label(&self, choice: CredentialChoice, cx: &App) -> String {
        let model = self.model.read(cx);
        match choice {
            CredentialChoice::Identity => self.credentials.identity.map(|id| {
                model
                    .identity(id)
                    .map_or_else(|| "A deleted identity".to_owned(), |i| i.title().to_owned())
            }),
            CredentialChoice::Key => self.credentials.key.map(|id| {
                model
                    .key(id)
                    .map_or_else(|| "A deleted key".to_owned(), |k| k.title().to_owned())
            }),
        }
        .unwrap_or_else(|| "Inherit".to_owned())
    }

    fn open_credential_picker(
        &mut self,
        choice: CredentialChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let inherit = (
            Credential::Inherit,
            "Inherit".to_owned(),
            "Use the group's, if it has one".to_owned(),
        );
        let (title, current, rest): (&str, Credential, Vec<(Credential, String, String)>) =
            match choice {
                CredentialChoice::Identity => (
                    "Identity",
                    self.credentials
                        .identity
                        .map_or(Credential::Inherit, Credential::Identity),
                    crate::keychain::identities::sorted(model.identities())
                        .into_iter()
                        .map(|i| {
                            let row = (i.title().to_owned(), i.username.clone());
                            (Credential::Identity(i.id), row.0, row.1)
                        })
                        .collect(),
                ),
                CredentialChoice::Key => (
                    "Key",
                    self.credentials
                        .key
                        .map_or(Credential::Inherit, Credential::Key),
                    crate::keychain::list::sorted(model.keys())
                        .into_iter()
                        .map(|k| {
                            (
                                Credential::Key(k.id),
                                k.title().to_owned(),
                                k.kind.display_name(),
                            )
                        })
                        .collect(),
                ),
            };
        let choices: Vec<(Credential, String, String)> =
            std::iter::once(inherit).chain(rest).collect();
        let items = choices
            .iter()
            .map(|(_, label, detail)| PickerItem::new(label.clone()).detail(detail.clone()))
            .collect();
        let selected = choices
            .iter()
            .position(|(c, ..)| *c == current)
            .unwrap_or(0);
        let picker = cx.new(|cx| Picker::new(title, items, window, cx).with_selected(selected));
        let subscription = cx.subscribe_in(&picker, window, move |editor, _, event, window, cx| {
            if let PickerEvent::Picked(index) = event
                && let Some((picked, ..)) = choices.get(*index)
            {
                match (choice, *picked) {
                    (CredentialChoice::Identity, Credential::Identity(id)) => {
                        editor.credentials.identity = Some(id);
                    }
                    (CredentialChoice::Key, Credential::Key(id)) => {
                        editor.credentials.key = Some(id);
                    }
                    (CredentialChoice::Identity, _) => editor.credentials.identity = None,
                    (CredentialChoice::Key, _) => editor.credentials.key = None,
                }
                editor.update_placeholders(cx);
            }
            editor.picker = None;
            match choice {
                CredentialChoice::Identity => editor.identity_focus.focus(window),
                CredentialChoice::Key => editor.key_focus.focus(window),
            }
            cx.notify();
        });
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    /// Notes a settings problem against its field.
    fn settings_error(&mut self, error: &SettingsError, settings: &HostSettings) {
        let field = match error {
            SettingsError::EmptyUsername | SettingsError::InvalidUsername(..) => Field::Username,
            SettingsError::InvalidAlgorithmName(name) => {
                algorithm_field(&settings.algorithms, name)
            }
        };
        self.fail(field, error.to_string());
    }

    /// Checks the algorithm names are ones SSHerlock supports (CONN-17).
    fn check_algorithms(&mut self, settings: &HostSettings) {
        if let Err(SshError::UnknownAlgorithm { kind, name }) =
            ssherlock_ssh::algorithms::validate(&settings.algorithms)
        {
            let field = match kind {
                "key exchange" => Field::Kex,
                "host key" => Field::HostKey,
                "MAC" => Field::Mac,
                _ => Field::Cipher,
            };
            self.fail(
                field,
                format!("SSHerlock doesn't support the {kind} algorithm {name}"),
            );
        }
    }

    fn save(&mut self, _: &SaveEditor, _: &mut Window, cx: &mut Context<Self>) {
        self.errors.clear();
        let saved = match self.mode.clone() {
            EditorMode::NewHost { .. } => self.save_host(None, cx),
            EditorMode::EditHost(host) => self.save_host(Some(host), cx),
            EditorMode::NewGroup { .. } => self.save_group(None, cx),
            EditorMode::EditGroup(group) => self.save_group(Some(group), cx),
            EditorMode::Bulk(ids) => self.save_bulk(&ids, cx),
        };
        match saved {
            Some(saved) if self.errors.is_empty() => cx.emit(HostEditorEvent::Done(Some(saved))),
            _ => cx.notify(),
        }
    }

    fn save_host(&mut self, original: Option<Host>, cx: &mut Context<Self>) -> Option<Saved> {
        let now = Timestamp::now();
        let settings = self.read_settings(cx);
        let address = Self::text(&self.inputs.address, cx);
        let address: Option<Address> = match address.trim().parse() {
            Ok(address) => Some(address),
            Err(error) => {
                self.fail(Field::Address, error.to_string());
                None
            }
        };
        let tags = match Tag::parse_list(&Self::text(&self.inputs.tags, cx)) {
            Ok(tags) => tags,
            Err(error) => {
                self.fail(Field::Tags, error.to_string());
                std::collections::BTreeSet::default()
            }
        };
        let address = address?;
        let mut host = match original {
            Some(host) => Host {
                address,
                updated_at: now,
                ..host
            },
            None => Host::new(address, now),
        };
        Self::text(&self.inputs.name, cx)
            .trim()
            .clone_into(&mut host.label);
        host.group = match self.group {
            Change::Set(group) => group,
            Change::Keep => host.group,
        };
        host.tags = tags;
        host.notes = Self::text(&self.inputs.notes, cx);
        host.is_favourite = match self.is_favourite {
            Change::Set(value) => value,
            Change::Keep => host.is_favourite,
        };
        host.settings = settings;
        match host.validate() {
            Ok(()) => {}
            Err(HostError::Settings(error)) => self.settings_error(&error, &host.settings),
            Err(error) => self.fail(Field::Name, error.to_string()),
        }
        self.check_algorithms(&host.settings);
        if !self.errors.is_empty() {
            return None;
        }
        let id = host.id;
        self.model
            .update(cx, |model, cx| model.save_hosts(vec![host], cx));
        Some(Saved::Host(id))
    }

    fn save_group(&mut self, original: Option<Group>, cx: &mut Context<Self>) -> Option<Saved> {
        let now = Timestamp::now();
        let settings = self.read_settings(cx);
        let name = Self::text(&self.inputs.name, cx).trim().to_owned();
        let parent = match self.group {
            Change::Set(parent) => parent,
            Change::Keep => None,
        };
        let group = match original {
            Some(group) => Group {
                name,
                parent,
                settings,
                updated_at: now,
                ..group
            },
            None => Group {
                settings,
                ..Group::new(name, parent, now)
            },
        };
        match group.validate() {
            Ok(()) => {}
            Err(GroupError::Settings(error)) => self.settings_error(&error, &group.settings),
            Err(error) => self.fail(Field::Name, error.to_string()),
        }
        let groups = self.known_groups(cx);
        if let Err(error) = GroupTree::new(&groups).check_parent(group.id, group.parent) {
            self.fail(Field::Group, error.to_string());
        }
        self.check_algorithms(&group.settings);
        if !self.errors.is_empty() {
            return None;
        }
        let id = group.id;
        self.model
            .update(cx, |model, cx| model.save_group(group, cx));
        Some(Saved::Group(id))
    }

    fn save_bulk(&mut self, ids: &[Id<Host>], cx: &mut Context<Self>) -> Option<Saved> {
        let now = Timestamp::now();
        let add_tags = self.parse_tags(&self.inputs.tags.clone(), Field::Tags, cx);
        let remove_tags = self.parse_tags(&self.inputs.remove_tags.clone(), Field::RemoveTags, cx);
        let port = match fields::port(&Self::text(&self.inputs.port, cx)) {
            Ok(Some(port)) => Change::Set(Some(port)),
            Ok(None) => Change::Keep,
            Err(message) => {
                self.fail(Field::Port, message);
                Change::Keep
            }
        };
        let username = match fields::username(&Self::text(&self.inputs.username, cx)) {
            Some(user) => {
                let check = HostSettings {
                    username: Some(user.clone()),
                    ..HostSettings::default()
                };
                if let Err(error) = check.validate() {
                    self.settings_error(&error, &check);
                }
                Change::Set(Some(user))
            }
            None => Change::Keep,
        };
        let edit = BulkEdit {
            group: self.group.clone(),
            add_tags,
            remove_tags,
            username,
            port,
            colour: self.colour.clone(),
            is_favourite: self.is_favourite.clone(),
        };
        if !self.errors.is_empty() {
            return None;
        }
        let model = self.model.read(cx);
        let hosts: Vec<Host> = ids
            .iter()
            .filter_map(|id| model.host(*id))
            .map(|host| edit.apply(host, now))
            .collect();
        if !edit.is_empty() {
            self.model
                .update(cx, |model, cx| model.save_hosts(hosts, cx));
        }
        Some(Saved::Hosts(ids.to_vec()))
    }

    fn parse_tags(
        &mut self,
        input: &Entity<TextInput>,
        field: Field,
        cx: &App,
    ) -> std::collections::BTreeSet<Tag> {
        Tag::parse_list(&Self::text(input, cx)).unwrap_or_else(|error| {
            self.fail(field, error.to_string());
            std::collections::BTreeSet::new()
        })
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn cancel(&mut self, _: &CancelEditor, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(HostEditorEvent::Done(None));
    }

    /// What the editor says at the top: "New host", "Edit 3 hosts", …
    pub fn title(&self) -> String {
        match &self.mode {
            EditorMode::NewHost { .. } => "New host".to_owned(),
            EditorMode::EditHost(host) => format!("Edit {}", host.title()),
            EditorMode::NewGroup { .. } => "New group".to_owned(),
            EditorMode::EditGroup(group) => format!("Edit group {}", group.name),
            EditorMode::Bulk(ids) => format!("Edit {} hosts", ids.len()),
        }
    }

    /// The error shown under `field`, if any; for tests.
    #[cfg(test)]
    fn error(&self, field: Field) -> Option<&str> {
        self.errors.get(&field).map(AsRef::as_ref)
    }
}

/// Which algorithm list holds `name`.
fn algorithm_field(algorithms: &AlgorithmPreferences, name: &str) -> Field {
    let has = |list: &[String]| list.iter().any(|n| n == name);
    if has(&algorithms.kex) {
        Field::Kex
    } else if has(&algorithms.host_key) {
        Field::HostKey
    } else if has(&algorithms.mac) {
        Field::Mac
    } else {
        Field::Cipher
    }
}

/// The colour choices: Keep (bulk only), Inherit, then each colour.
fn colour_choices(is_bulk: bool) -> Vec<Change<Option<Colour>>> {
    let mut choices = Vec::new();
    if is_bulk {
        choices.push(Change::Keep);
    }
    choices.push(Change::Set(None));
    choices.extend(Colour::ALL.into_iter().map(|c| Change::Set(Some(c))));
    choices
}

const COMPRESSION_CHOICES: [Option<Compression>; 3] =
    [None, Some(Compression::Off), Some(Compression::On)];

const FAVOURITE_CHOICES: [Change<bool>; 3] = [Change::Keep, Change::Set(true), Change::Set(false)];

impl Render for HostEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let is_bulk = self.is_bulk();
        let is_group = self.is_group();
        let title = self.title();
        let error = |field: Field| self.errors.get(&field).cloned();
        let inherited = self.inherited(cx);
        let model = self.model.read(cx);
        let tree = GroupTree::new(model.groups());
        let source_text = |source: Source| match source {
            Source::Group(id) => tree
                .get(id)
                .map_or_else(String::new, |g| format!(" from {}", g.name)),
            _ => String::new(),
        };

        let group_button = Button::new("group", self.group_label(cx))
            .dropdown()
            .focus_handle(&self.group_focus)
            .on_click(cx.listener(|editor, _, window, cx| {
                editor.open_group_picker(window, cx);
            }));
        let group_label = if is_group { "Inside" } else { "Group" };

        // Colour: Keep (bulk), Inherit, then the nine marks.
        let colours = colour_choices(is_bulk);
        let colour_index = colours.iter().position(|c| *c == self.colour).unwrap_or(0);
        let colour_segments = colours
            .iter()
            .map(|choice| match choice {
                Change::Keep => Segment::new("Keep"),
                Change::Set(None) => Segment::new("Inherit"),
                Change::Set(Some(colour)) => {
                    Segment::swatch(colour.name(), theme.colors.marks.get(*colour))
                }
            })
            .collect();
        let colour_control =
            SegmentedControl::new("colour", colour_segments, colour_index, &self.colour_focus)
                .on_select(cx.listener(move |editor, index: &usize, _, cx| {
                    if let Some(choice) = colour_choices(editor.is_bulk()).get(*index) {
                        editor.colour = choice.clone();
                        cx.notify();
                    }
                }));
        let colour_hint = match (&self.colour, &inherited) {
            (Change::Set(Some(colour)), _) => Some(colour.name().to_owned()),
            (Change::Set(None), Some(inherited)) => Some(match inherited.colour.value {
                Some(colour) => format!(
                    "Inherits {}{}",
                    colour.name(),
                    source_text(inherited.colour.source)
                ),
                None => "No colour".to_owned(),
            }),
            _ => None,
        };

        let connection = FormSection::new(if is_group {
            "Connection, for everything inside"
        } else {
            "Connection"
        })
        .child(FormField::new("Port", self.inputs.port.clone()).error(error(Field::Port)))
        .child(
            FormField::new("User name", self.inputs.username.clone()).error(error(Field::Username)),
        )
        .child({
            let field = FormField::new("Colour", colour_control);
            match colour_hint {
                Some(hint) => field.hint(hint),
                None => field,
            }
        })
        .when(!is_bulk, |section| {
            let index = COMPRESSION_CHOICES
                .iter()
                .position(|c| *c == self.compression)
                .unwrap_or(0);
            let hint = match (&self.compression, &inherited) {
                (None, Some(inherited)) => format!(
                    "Inherits {}{}",
                    match inherited.compression.value {
                        Compression::Off => "off",
                        Compression::On => "on",
                    },
                    source_text(inherited.compression.source)
                ),
                _ => "Worth it only on slow links".to_owned(),
            };
            section.child(
                FormField::new(
                    "Compression",
                    SegmentedControl::new(
                        "compression",
                        vec![
                            Segment::new("Inherit"),
                            Segment::new("Off"),
                            Segment::new("On"),
                        ],
                        index,
                        &self.compression_focus,
                    )
                    .on_select(cx.listener(|editor, index: &usize, _, cx| {
                        if let Some(choice) = COMPRESSION_CHOICES.get(*index) {
                            editor.compression = *choice;
                            cx.notify();
                        }
                    })),
                )
                .hint(hint),
            )
        });

        let form = if is_bulk {
            let favourite_index = FAVOURITE_CHOICES
                .iter()
                .position(|c| *c == self.is_favourite)
                .unwrap_or(0);
            Form::new()
                .child(
                    FormSection::new("Change every selected host")
                        .child(FormField::new(group_label, group_button).error(error(Field::Group)))
                        .child(
                            FormField::new("Add tags", self.inputs.tags.clone())
                                .error(error(Field::Tags)),
                        )
                        .child(
                            FormField::new("Remove tags", self.inputs.remove_tags.clone())
                                .error(error(Field::RemoveTags)),
                        )
                        .child(FormField::new(
                            "Favourite",
                            SegmentedControl::new(
                                "favourite",
                                vec![
                                    Segment::new("Keep"),
                                    Segment::new("Yes"),
                                    Segment::new("No"),
                                ],
                                favourite_index,
                                &self.favourite_focus,
                            )
                            .on_select(cx.listener(
                                |editor, index: &usize, _, cx| {
                                    if let Some(choice) = FAVOURITE_CHOICES.get(*index) {
                                        editor.is_favourite = choice.clone();
                                        cx.notify();
                                    }
                                },
                            )),
                        )),
                )
                .child(connection)
        } else {
            let main = if is_group {
                FormSection::new("Group")
                    .child(
                        FormField::new("Name", self.inputs.name.clone()).error(error(Field::Name)),
                    )
                    .child(FormField::new(group_label, group_button).error(error(Field::Group)))
            } else {
                let is_favourite = self.is_favourite == Change::Set(true);
                FormSection::new("Host")
                    .child(
                        FormField::new("Address", self.inputs.address.clone())
                            .error(error(Field::Address)),
                    )
                    .child(
                        FormField::new("Label", self.inputs.name.clone()).error(error(Field::Name)),
                    )
                    .child(FormField::new(group_label, group_button).error(error(Field::Group)))
                    .child(
                        FormField::new("Tags", self.inputs.tags.clone())
                            .hint("Separate with commas or spaces")
                            .error(error(Field::Tags)),
                    )
                    .child(FormField::new(
                        "Favourite",
                        Checkbox::new(
                            "favourite",
                            "Show under Favourites",
                            is_favourite,
                            &self.favourite_focus,
                        )
                        .on_toggle(cx.listener(
                            |editor, value: &bool, _, cx| {
                                editor.is_favourite = Change::Set(*value);
                                cx.notify();
                            },
                        )),
                    ))
            };
            let algorithms = FormSection::new("Algorithms")
                .child(algorithm_field_row(
                    "Key exchange",
                    &self.inputs.kex,
                    error(Field::Kex),
                ))
                .child(algorithm_field_row(
                    "Host keys",
                    &self.inputs.host_key,
                    error(Field::HostKey),
                ))
                .child(algorithm_field_row(
                    "Ciphers",
                    &self.inputs.cipher,
                    error(Field::Cipher),
                ))
                .child(algorithm_field_row(
                    "MACs",
                    &self.inputs.mac,
                    error(Field::Mac),
                ));
            let credential_hint = |own: bool, inherited: Option<String>| {
                if own {
                    None
                } else {
                    Some(inherited.unwrap_or_else(|| "None".to_owned()))
                }
            };
            let identity_hint = credential_hint(
                self.credentials.identity.is_some(),
                inherited.as_ref().and_then(|inherited| {
                    let id = inherited.identity.value?;
                    let title = model.identity(id)?.title().to_owned();
                    Some(format!(
                        "Inherits {title}{}",
                        source_text(inherited.identity.source)
                    ))
                }),
            );
            let key_hint = credential_hint(
                self.credentials.key.is_some(),
                inherited.as_ref().and_then(|inherited| {
                    let id = inherited.key.value?;
                    let title = model.key(id)?.title().to_owned();
                    Some(format!(
                        "Inherits {title}{}",
                        source_text(inherited.key.source)
                    ))
                }),
            );
            let with_hint = |field: FormField, hint: Option<String>| match hint {
                Some(hint) => field.hint(hint),
                None => field,
            };
            let credentials = FormSection::new(if is_group {
                "Credentials, for everything inside"
            } else {
                "Credentials"
            })
            .child(with_hint(
                FormField::new(
                    "Identity",
                    Button::new(
                        "identity",
                        self.credential_label(CredentialChoice::Identity, cx),
                    )
                    .dropdown()
                    .focus_handle(&self.identity_focus)
                    .on_click(cx.listener(|editor, _, window, cx| {
                        editor.open_credential_picker(CredentialChoice::Identity, window, cx);
                    })),
                ),
                identity_hint,
            ))
            .child(with_hint(
                FormField::new(
                    "Key",
                    Button::new("key", self.credential_label(CredentialChoice::Key, cx))
                        .dropdown()
                        .focus_handle(&self.key_focus)
                        .on_click(cx.listener(|editor, _, window, cx| {
                            editor.open_credential_picker(CredentialChoice::Key, window, cx);
                        })),
                ),
                key_hint,
            ))
            .child(
                FormField::new("Password", self.inputs.password.clone())
                    .error(error(Field::Password)),
            )
            .when(self.credentials.password.is_some(), |section| {
                section.child(FormField::new(
                    "",
                    Checkbox::new(
                        "forget-password",
                        "Forget the saved password",
                        self.forget_password,
                        &self.forget_focus,
                    )
                    .on_toggle(cx.listener(|editor, value: &bool, _, cx| {
                        editor.forget_password = *value;
                        cx.notify();
                    })),
                ))
            });
            Form::new()
                .child(main)
                .child(connection)
                .child(credentials)
                .child(algorithms)
                .when(!is_group, |form| {
                    form.child(FormSection::new("Notes").child(self.inputs.notes.clone()))
                })
        };

        sheet(
            &theme,
            title,
            "Save",
            form.into_any_element(),
            cx.listener(|editor, _, window, cx| editor.cancel(&CancelEditor, window, cx)),
            cx.listener(|editor, _, window, cx| editor.save(&SaveEditor, window, cx)),
        )
        .key_context("HostEditor")
        .track_focus(&self.focus_handle)
        .on_action(cx.listener(Self::save))
        .on_action(cx.listener(Self::cancel))
        .when_some(self.picker.as_ref(), |d, (picker, _)| {
            d.child(picker.clone())
        })
    }
}

fn algorithm_field_row(
    label: &'static str,
    input: &Entity<TextInput>,
    error: Option<SharedString>,
) -> FormField {
    FormField::new(label, input.clone())
        .hint("Most preferred first, separated by commas")
        .error(error)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui::{TestAppContext, VisualTestContext};
    use pretty_assertions::assert_eq;
    use ssherlock_core::{ExposeSecret as _, Port};
    use ssherlock_store::{Repository as _, Store};

    use super::*;

    struct Fixture {
        editor: Entity<HostEditor>,
        model: Entity<HostsModel>,
        events: Rc<RefCell<Vec<HostEditorEvent>>>,
        _subscription: Subscription,
    }

    fn open<'a>(
        mode: impl FnOnce(&[Group]) -> EditorMode,
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
        let mode = mode(groups);
        let (editor, cx) =
            cx.add_window_view(|window, cx| HostEditor::new(model.clone(), mode, window, cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let subscription = cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &HostEditorEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        (
            Fixture {
                editor,
                model,
                events,
                _subscription: subscription,
            },
            cx,
        )
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    #[gpui::test]
    fn credentials_come_from_an_identity_and_a_typed_password_is_sealed(cx: &mut TestAppContext) {
        let (f, cx) = open(|_| EditorMode::NewHost { group: None }, &[], &[], cx);
        let mut identity = Identity::new("deploy", at(1));
        identity.label = "Deploy".into();
        let saved_identity = identity.clone();
        f.model
            .update(cx, |m, cx| m.save_identity(saved_identity, cx));
        cx.run_until_parked();

        // Pick the identity: the user name placeholder now says where it comes from.
        f.editor.update_in(cx, |editor, window, cx| {
            editor.open_credential_picker(CredentialChoice::Identity, window, cx);
        });
        let picker = f
            .editor
            .read_with(cx, |e, _| e.picker.as_ref().map(|(p, _)| p.clone()))
            .unwrap();
        picker.update(cx, |_, cx| cx.emit(PickerEvent::Picked(1)));
        cx.run_until_parked();
        let placeholder = |cx: &mut VisualTestContext| {
            f.editor.read_with(cx, |editor, cx| {
                editor
                    .inputs
                    .username
                    .read(cx)
                    .placeholder_text()
                    .map(ToString::to_string)
            })
        };
        assert_eq!(
            placeholder(cx).as_deref(),
            Some("deploy (from identity Deploy)")
        );

        let set =
            |input: fn(&Inputs) -> &Entity<TextInput>, text: &str, cx: &mut VisualTestContext| {
                let input = f.editor.read_with(cx, |e, _| input(&e.inputs).clone());
                input.update(cx, |i, cx| i.set_text(text, cx));
            };
        set(|i| &i.address, "db.example.com", cx);
        set(|i| &i.password, "hunter2", cx);
        // Focus is on the identity button, where Enter would open its picker again.
        f.editor.update_in(cx, |editor, window, cx| {
            editor.save(&SaveEditor, window, cx);
        });
        cx.run_until_parked();
        let host = f
            .model
            .read_with(cx, |m, _| m.hosts().first().cloned())
            .unwrap();
        assert_eq!(host.settings.identity, Some(identity.id));
        let vault = f.model.read_with(cx, |m, _| m.vault().cloned()).unwrap();
        let sealed = host.settings.password.clone().unwrap();
        assert_eq!(
            vault.open_string(&sealed).unwrap().expose_secret(),
            "hunter2"
        );
    }

    /// Regression: picking with Enter returned focus to the dropdown button, and the key's
    /// release clicked it, opening the picker again.
    #[gpui::test]
    fn picking_with_enter_does_not_reopen_the_picker(cx: &mut TestAppContext) {
        let (f, cx) = open(|_| EditorMode::NewHost { group: None }, &[], &[], cx);
        let is_open =
            |cx: &mut VisualTestContext| f.editor.read_with(cx, |e, _| e.picker.is_some());
        // A full press: gpui clicks a focused element on the key's release.
        let press = |cx: &mut VisualTestContext| {
            cx.simulate_keystrokes("enter");
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("enter").unwrap(),
            });
            cx.run_until_parked();
        };
        f.editor
            .update_in(cx, |editor, window, _| editor.group_focus.focus(window));
        press(cx);
        assert!(is_open(cx), "Enter on the focused button opens the picker");
        press(cx);
        assert!(
            !is_open(cx),
            "Enter picks and closes it, and its release does nothing"
        );
    }

    #[gpui::test]
    fn a_saved_password_is_kept_unless_forgotten(cx: &mut TestAppContext) {
        let mut host = Host::new("example.com".parse().unwrap(), at(1));
        host.settings.password = Some(Sealed::from_bytes(vec![9; 40]));
        let original = host.clone();
        let (f, cx) = open(
            move |_| EditorMode::EditHost(original),
            &[host.clone()],
            &[],
            cx,
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let saved = f
            .model
            .read_with(cx, |m, _| m.host(host.id).cloned())
            .unwrap();
        assert_eq!(saved.settings.password, host.settings.password, "kept");

        let original = saved.clone();
        let (f, cx) = open(move |_| EditorMode::EditHost(original), &[saved], &[], cx);
        f.editor
            .update(cx, |editor, _| editor.forget_password = true);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let forgotten = f
            .model
            .read_with(cx, |m, _| m.host(host.id).cloned())
            .unwrap();
        assert_eq!(forgotten.settings.password, None);
    }

    #[gpui::test]
    fn a_new_host_is_typed_tabbed_through_and_saved(cx: &mut TestAppContext) {
        let (f, cx) = open(|_| EditorMode::NewHost { group: None }, &[], &[], cx);
        cx.simulate_input("db.example.com");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("Prod DB");
        // Group button, then tags.
        cx.simulate_keystrokes("tab tab");
        cx.simulate_input("prod, db");
        // Favourite checkbox, then port.
        cx.simulate_keystrokes("tab");
        crate::components::form::tests::press(cx, "space");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("2222");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        let events = f.events.take();
        assert!(
            matches!(
                events.as_slice(),
                [HostEditorEvent::Done(Some(Saved::Host(_)))]
            ),
            "{events:?}"
        );
        let hosts = f.model.read_with(cx, |m, _| m.hosts().to_vec());
        let [host] = hosts.as_slice() else {
            unreachable!("one host, got {hosts:?}")
        };
        assert_eq!(host.address.to_string(), "db.example.com");
        assert_eq!(host.label, "Prod DB");
        assert_eq!(Tag::format_list(&host.tags), "db, prod");
        assert!(host.is_favourite);
        assert_eq!(host.settings.port, Port::new(2222));
    }

    #[gpui::test]
    fn problems_show_under_their_fields_and_nothing_is_saved(cx: &mut TestAppContext) {
        let (f, cx) = open(|_| EditorMode::NewHost { group: None }, &[], &[], cx);
        // No address; a bad port and an unknown cipher.
        f.editor.update(cx, |editor, cx| {
            editor
                .inputs
                .port
                .update(cx, |i, cx| i.set_text("99999", cx));
            editor
                .inputs
                .cipher
                .update(cx, |i, cx| i.set_text("rot13", cx));
            editor
                .inputs
                .username
                .update(cx, |i, cx| i.set_text("al ice", cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        f.editor.read_with(cx, |editor, _| {
            assert_eq!(editor.error(Field::Address), Some("The address is empty"));
            assert_eq!(
                editor.error(Field::Port),
                Some("Use a number from 1 to 65535")
            );
            assert_eq!(
                editor.error(Field::Cipher),
                None,
                "checked only once the address is valid"
            );
        });
        f.editor.update(cx, |editor, cx| {
            editor
                .inputs
                .address
                .update(cx, |i, cx| i.set_text("example.com", cx));
            editor.inputs.port.update(cx, |i, cx| i.set_text("", cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        f.editor.read_with(cx, |editor, _| {
            assert!(
                editor
                    .error(Field::Username)
                    .is_some_and(|e| e.contains("al ice"))
            );
            assert_eq!(
                editor.error(Field::Cipher),
                Some("SSHerlock doesn't support the cipher algorithm rot13")
            );
        });
        assert_eq!(f.events.take(), []);
        assert_eq!(f.model.read_with(cx, |m, _| m.hosts().len()), 0);
        cx.simulate_keystrokes("escape");
        assert_eq!(f.events.take(), [HostEditorEvent::Done(None)]);
    }

    #[gpui::test]
    fn editing_keeps_what_the_form_doesnt_show(cx: &mut TestAppContext) {
        let mut host = Host::new("old.example.com".parse().unwrap(), at(1));
        host.last_connected_at = Some(at(5));
        host.settings.algorithms.kex = vec!["curve25519-sha256".into()];
        host.settings.key = Some(Id::generate());
        host.settings.identity = Some(Id::generate());
        host.settings.password = Some(Sealed::from_bytes(vec![1, 2, 3]));
        let id = host.id;
        let original = host.clone();
        let credentials = host.settings.clone();
        let (f, cx) = open(move |_| EditorMode::EditHost(original), &[host], &[], cx);
        f.editor.update(cx, |editor, cx| {
            editor
                .inputs
                .address
                .update(cx, |i, cx| i.set_text("new.example.com", cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let saved = f.model.read_with(cx, |m, _| m.host(id).cloned()).unwrap();
        assert_eq!(saved.address.to_string(), "new.example.com");
        assert_eq!(saved.created_at, at(1));
        assert_eq!(saved.last_connected_at, Some(at(5)));
        assert_eq!(saved.settings.algorithms.kex, ["curve25519-sha256"]);
        assert_eq!(
            (
                saved.settings.key,
                saved.settings.identity,
                saved.settings.password
            ),
            (credentials.key, credentials.identity, credentials.password)
        );
    }

    #[gpui::test]
    fn placeholders_show_what_is_inherited(cx: &mut TestAppContext) {
        let mut prod = Group::new("Production", None, at(1));
        prod.settings.username = Some("ops".into());
        prod.settings.port = Port::new(2200);
        let (f, cx) = open(
            |groups| EditorMode::NewHost {
                group: groups.first().map(|g| g.id),
            },
            &[],
            &[prod],
            cx,
        );
        let placeholder = |input: fn(&Inputs) -> &Entity<TextInput>, cx: &mut VisualTestContext| {
            f.editor.read_with(cx, |editor, cx| {
                input(&editor.inputs)
                    .read(cx)
                    .placeholder_text()
                    .map(ToString::to_string)
            })
        };
        assert_eq!(
            placeholder(|i| &i.username, cx).as_deref(),
            Some("ops (from Production)")
        );
        assert_eq!(
            placeholder(|i| &i.port, cx).as_deref(),
            Some("2200 (from Production)")
        );
        assert_eq!(
            placeholder(|i| &i.cipher, cx).as_deref(),
            Some("SSHerlock's defaults")
        );
    }

    #[gpui::test]
    fn a_group_cannot_move_inside_itself_and_saves_its_settings(cx: &mut TestAppContext) {
        let outer = Group::new("outer", None, at(1));
        let inner = Group::new("inner", Some(outer.id), at(1));
        let editing = outer.clone();
        let (f, cx) = open(
            move |_| EditorMode::EditGroup(editing),
            &[],
            &[outer.clone(), inner.clone()],
            cx,
        );
        let offered: Vec<String> = f.editor.read_with(cx, |editor, cx| {
            editor
                .group_choices(cx)
                .into_iter()
                .map(|(_, label)| label)
                .collect()
        });
        assert_eq!(
            offered,
            ["Top level"],
            "neither itself nor its subgroup is offered"
        );

        f.editor.update(cx, |editor, cx| {
            editor
                .inputs
                .username
                .update(cx, |i, cx| i.set_text("deploy", cx));
            editor.compression = Some(Compression::On);
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let saved = f
            .model
            .read_with(cx, |m, _| {
                m.groups().iter().find(|g| g.id == outer.id).cloned()
            })
            .unwrap();
        assert_eq!(saved.settings.username.as_deref(), Some("deploy"));
        assert_eq!(saved.settings.compression, Some(Compression::On));
    }

    #[gpui::test]
    fn a_group_can_be_created_from_the_group_picker(cx: &mut TestAppContext) {
        let (f, cx) = open(|_| EditorMode::NewHost { group: None }, &[], &[], cx);
        cx.simulate_input("new.example.com");
        cx.simulate_keystrokes("tab tab");
        crate::components::form::tests::press(cx, "enter");
        assert!(
            f.editor.read_with(cx, |e, _| e.picker.is_some()),
            "Enter opens the picker"
        );
        cx.simulate_input("Lab");
        cx.simulate_keystrokes("enter");
        let label = f.editor.read_with(cx, HostEditor::group_label);
        assert_eq!(label, "Lab", "chosen before the model has reloaded");
        // Focus is back on the Group button, where Enter presses it; Ctrl/Cmd+Enter saves.
        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();
        f.model.read_with(cx, |m, _| {
            let lab = m
                .groups()
                .iter()
                .find(|g| g.name == "Lab")
                .expect("the group was saved");
            assert_eq!(m.hosts().first().and_then(|h| h.group), Some(lab.id));
        });
    }

    #[gpui::test]
    fn bulk_edit_changes_only_what_was_filled_in(cx: &mut TestAppContext) {
        let mut a = Host::new("a.example.com".parse().unwrap(), at(1));
        a.tags = Tag::parse_list("old").unwrap();
        a.settings.username = Some("alice".into());
        let mut b = Host::new("b.example.com".parse().unwrap(), at(1));
        b.settings.port = Port::new(2222);
        let ids = vec![a.id, b.id];
        let (f, cx) = open(
            move |_| EditorMode::Bulk(ids),
            &[a.clone(), b.clone()],
            &[],
            cx,
        );
        cx.simulate_input("new");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("old");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let get = |id, cx: &mut VisualTestContext| {
            f.model.read_with(cx, |m, _| m.host(id).cloned()).unwrap()
        };
        let (a2, b2) = (get(a.id, cx), get(b.id, cx));
        assert_eq!(Tag::format_list(&a2.tags), "new");
        assert_eq!(Tag::format_list(&b2.tags), "new");
        assert_eq!(a2.settings.username.as_deref(), Some("alice"), "kept");
        assert_eq!(b2.settings.port, Port::new(2222), "kept");
    }
}
