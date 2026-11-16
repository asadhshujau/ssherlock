//! [`KeyList`]: the keys page. The saved keys in a list, the details of the one under the
//! cursor, and the ways to add, export and remove keys (KEY-01, 04…07, 13, 14).
//!
//! It holds view state only. Keys come from the shared [`HostsModel`], and every change goes
//! back through it. Its dialogs (the key form, pickers, confirmations) are drawn by the hosts
//! page over the whole page; see [`KeyList::overlay`].

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ssherlock_core::hosts::Host;
use ssherlock_core::keychain::{Key, KeyProtection, Usage, usage_of_key};
use ssherlock_core::{ExposeSecret as _, Id, SecretString, Timestamp};
use ssherlock_ssh::keys::{self, ExportFormat, FoundKey};
use zeroize::Zeroizing;

use super::actions::{
    CopyPublicKey, DeleteKey, EditKey, ExportKey, ExportKeyToHost, GenerateKey, ImportFromSshDir,
    ImportKey,
};
use super::detail::{KeyButtons, KeyFacts, key_panel};
use super::editor::{KeyEditor, KeyEditorEvent, KeyEditorMode, copy_text};
use crate::components::{
    Button, ButtonStyle, InputKind, Modal, ModalEvent, Picker, PickerEvent, PickerItem, TextInput,
    TextInputEvent,
};
use crate::hosts::HostsModel;
use crate::hosts::actions::{SelectFirst, SelectLast, SelectNext, SelectPrevious};
use crate::notify;
use crate::ssh::{Passphrases, SshEnvironment};
use crate::terminal::mono_family;
use crate::theme::{ActiveTheme as _, Theme};

/// What a key can be exported as, in the order the picker offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportChoice {
    /// The `.pub` line.
    PublicKey,
    /// The private key as stored (OpenSSH format).
    OpenSsh,
    /// The private key as PKCS#8 PEM.
    Pkcs8,
}

impl ExportChoice {
    /// Every choice, as the picker lists them.
    pub const ALL: [Self; 3] = [Self::PublicKey, Self::OpenSsh, Self::Pkcs8];

    fn label(self) -> &'static str {
        match self {
            Self::PublicKey => "Public key",
            Self::OpenSsh => "Private key, OpenSSH format",
            Self::Pkcs8 => "Private key, PKCS#8 PEM",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::PublicKey => "For authorized_keys, or to give to a server's admin",
            Self::OpenSsh => "What ssh and ssh-keygen use; keeps the passphrase",
            Self::Pkcs8 => "For tools that don't read OpenSSH keys",
        }
    }
}

/// The file name to suggest when exporting `key` as `choice`: the label made file-safe, else
/// `id_<type>`, as `ssh-keygen` would name it.
pub fn export_file_name(key: &Key, choice: ExportChoice) -> String {
    let base: String = key
        .label
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if base.trim_matches('_').is_empty() {
        format!("id_{}", key.kind.to_string().replace('-', "_"))
    } else {
        base
    };
    match choice {
        ExportChoice::PublicKey => format!("{base}.pub"),
        ExportChoice::OpenSsh => base,
        ExportChoice::Pkcs8 => format!("{base}.pem"),
    }
}

/// The start of a `SHA256:…` fingerprint, enough to tell keys apart in a narrow list.
pub fn short_fingerprint(fingerprint: &str) -> String {
    const SHOWN: usize = 19;
    if fingerprint.chars().count() <= SHOWN {
        fingerprint.to_owned()
    } else {
        let start: String = fingerprint.chars().take(SHOWN).collect();
        format!("{start}\u{2026}")
    }
}

/// The keys in the order the page lists them: by title, ignoring case, then by age.
pub fn sorted(keys: &[Key]) -> Vec<&Key> {
    let mut sorted: Vec<&Key> = keys.iter().collect();
    sorted.sort_by(|a, b| {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then(a.created_at.cmp(&b.created_at))
    });
    sorted
}

/// What a picker open over the page is choosing.
enum Choosing {
    Export(Id<Key>),
    Found(Vec<FoundKey>),
    Host(Id<Key>, Vec<Id<Host>>),
}

/// What the keys page asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyListEvent {
    /// Connect to `host` and add `key` to its `authorized_keys` (KEY-07).
    ExportToHost {
        /// The key to add.
        key: Id<Key>,
        /// The saved host to add it to.
        host: Id<Host>,
    },
}

/// A dialog open over the page.
enum Dialog {
    Editor(Entity<KeyEditor>),
    Picker(Entity<Picker>, Choosing),
    Delete(Entity<Modal>),
    Passphrase {
        modal: Entity<Modal>,
        input: Entity<TextInput>,
        key: Id<Key>,
    },
}

/// The keys page.
pub struct KeyList {
    model: Entity<HostsModel>,
    cursor: Option<Id<Key>>,
    /// What uses each key, worked out when the model changes rather than every frame.
    usage: HashMap<Id<Key>, Usage>,
    dialog: Option<(Dialog, Vec<Subscription>)>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<KeyListEvent> for KeyList {}

impl Focusable for KeyList {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.dialog {
            Some((Dialog::Editor(editor), _)) => editor.focus_handle(cx),
            Some((Dialog::Picker(picker, _), _)) => picker.focus_handle(cx),
            Some((Dialog::Delete(modal), _)) => modal.focus_handle(cx),
            Some((Dialog::Passphrase { input, .. }, _)) => input.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl KeyList {
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

    /// The key under the cursor.
    pub fn cursor(&self) -> Option<Id<Key>> {
        self.cursor
    }

    /// The ids of the keys in the order shown.
    pub fn visible_ids(&self, cx: &App) -> Vec<Id<Key>> {
        sorted(self.model.read(cx).keys())
            .into_iter()
            .map(|k| k.id)
            .collect()
    }

    /// The dialog over the page, for the hosts page to draw above everything.
    pub fn overlay(&self) -> Option<AnyElement> {
        self.dialog.as_ref().map(|(dialog, _)| match dialog {
            Dialog::Editor(editor) => editor.clone().into_any_element(),
            Dialog::Picker(picker, _) => picker.clone().into_any_element(),
            Dialog::Delete(modal) | Dialog::Passphrase { modal, .. } => {
                modal.clone().into_any_element()
            }
        })
    }

    /// The key form, if it's open; for tests.
    pub fn editor(&self) -> Option<&Entity<KeyEditor>> {
        match &self.dialog {
            Some((Dialog::Editor(editor), _)) => Some(editor),
            _ => None,
        }
    }

    /// The delete confirmation, if it's open; for tests.
    pub fn delete_dialog(&self) -> Option<Entity<Modal>> {
        match &self.dialog {
            Some((Dialog::Delete(modal), _)) => Some(modal.clone()),
            _ => None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        self.usage = model
            .keys()
            .iter()
            .map(|k| {
                (
                    k.id,
                    usage_of_key(k.id, model.hosts(), model.groups(), model.identities()),
                )
            })
            .collect();
        let ids = self.visible_ids(cx);
        if self.cursor.is_none_or(|id| !ids.contains(&id)) {
            self.cursor = ids.first().copied();
        }
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.focus_handle.focus(window);
        cx.notify();
    }

    // ── cursor ─────────────────────────────────────────────────────────────────────────

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.visible_ids(cx);
        let Some(last) = ids.len().checked_sub(1) else {
            return;
        };
        let at = self
            .cursor
            .and_then(|id| ids.iter().position(|k| *k == id))
            .unwrap_or(0);
        let next = at.saturating_add_signed(delta).min(last);
        self.move_to(ids.get(next).copied(), next, cx);
    }

    fn move_to(&mut self, id: Option<Id<Key>>, position: usize, cx: &mut Context<Self>) {
        self.cursor = id;
        self.scroll.scroll_to_item(position);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        let first = self.visible_ids(cx).first().copied();
        self.move_to(first, 0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.visible_ids(cx);
        self.move_to(ids.last().copied(), ids.len().saturating_sub(1), cx);
    }

    fn cursor_key(&self, cx: &App) -> Option<Key> {
        self.cursor
            .and_then(|id| self.model.read(cx).key(id))
            .cloned()
    }

    // ── the key form ───────────────────────────────────────────────────────────────────

    fn open_editor(&mut self, mode: KeyEditorMode, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let editor = cx.new(|cx| KeyEditor::new(model, mode, window, cx));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |list, _, event: &KeyEditorEvent, window, cx| {
                let KeyEditorEvent::Done(saved) = event;
                if let Some(id) = saved {
                    list.cursor = Some(*id);
                }
                list.close_dialog(window, cx);
            },
        );
        self.dialog = Some((Dialog::Editor(editor), vec![subscription]));
        cx.notify();
    }

    fn generate(&mut self, _: &GenerateKey, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(KeyEditorMode::Generate, window, cx);
    }

    fn import(&mut self, _: &ImportKey, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(KeyEditorMode::Import { path: None }, window, cx);
    }

    fn edit(&mut self, _: &EditKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.cursor_key(cx) {
            self.open_editor(KeyEditorMode::Edit(key), window, cx);
        }
    }

    // ── importing from ~/.ssh ──────────────────────────────────────────────────────────

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn import_from_ssh_dir(
        &mut self,
        _: &ImportFromSshDir,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = cx
            .try_global::<SshEnvironment>()
            .and_then(|env| env.ssh_dir.clone())
        else {
            notify::info("There is no ~/.ssh folder to look in", cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let scan_dir = dir.clone();
            let found = cx
                .background_spawn(async move { keys::scan_blocking(&scan_dir) })
                .await;
            let _ = this.update_in(cx, |list, window, cx| {
                list.show_found(found, &dir, window, cx);
            });
        })
        .detach();
    }

    fn show_found(
        &mut self,
        found: Vec<FoundKey>,
        dir: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if found.is_empty() {
            notify::info(format!("No private keys in {}", dir.display()), cx);
            return;
        }
        let model = self.model.read(cx);
        let items = found
            .iter()
            .map(|key| {
                let name = key
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let saved = key
                    .fingerprint
                    .as_ref()
                    .and_then(|print| model.keys().iter().find(|k| &k.fingerprint == print));
                let detail = match (saved, &key.fingerprint) {
                    (Some(saved), _) => format!("Already saved as {}", saved.title()),
                    (None, Some(print)) => short_fingerprint(print),
                    (None, None) => "Encrypted; needs its passphrase".to_owned(),
                };
                PickerItem::new(name).detail(detail)
            })
            .collect();
        let picker = cx.new(|cx| Picker::new("Import a key from ~/.ssh", items, window, cx));
        let subscription = cx.subscribe_in(&picker, window, Self::picker_event);
        self.dialog = Some((
            Dialog::Picker(picker, Choosing::Found(found)),
            vec![subscription],
        ));
        cx.notify();
    }

    fn picker_event(
        &mut self,
        _: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((Dialog::Picker(_, choosing), _)) = self.dialog.take() else {
            return;
        };
        self.close_dialog(window, cx);
        let PickerEvent::Picked(index) = event else {
            return;
        };
        match choosing {
            Choosing::Found(found) => {
                if let Some(key) = found.into_iter().nth(*index) {
                    self.open_editor(
                        KeyEditorMode::Import {
                            path: Some(key.path),
                        },
                        window,
                        cx,
                    );
                }
            }
            Choosing::Export(id) => {
                if let Some(choice) = ExportChoice::ALL.get(*index) {
                    self.export_as(id, *choice, None, window, cx);
                }
            }
            Choosing::Host(key, hosts) => {
                if let Some(host) = hosts.get(*index) {
                    cx.emit(KeyListEvent::ExportToHost { key, host: *host });
                }
            }
        }
    }

    // ── exporting ──────────────────────────────────────────────────────────────────────

    fn export_to_host(&mut self, _: &ExportKeyToHost, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.cursor_key(cx) else {
            return;
        };
        let model = self.model.read(cx);
        let mut hosts: Vec<&Host> = model.hosts().iter().collect();
        hosts.sort_by_key(|h| h.title().to_lowercase());
        if hosts.is_empty() {
            notify::info("There are no saved hosts to add the key to", cx);
            return;
        }
        let items = hosts
            .iter()
            .map(|h| PickerItem::new(h.title().into_owned()).detail(h.address.to_string()))
            .collect();
        let ids = hosts.iter().map(|h| h.id).collect();
        let picker = cx.new(|cx| {
            Picker::new(
                format!("Add {} to a host's authorized_keys", key.title()),
                items,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(&picker, window, Self::picker_event);
        self.dialog = Some((
            Dialog::Picker(picker, Choosing::Host(key.id, ids)),
            vec![subscription],
        ));
        cx.notify();
    }

    fn copy_public_key(&mut self, _: &CopyPublicKey, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.cursor_key(cx) {
            copy_text(&key.public_key, cx);
            notify::info(format!("Copied the public key of {}", key.title()), cx);
        }
    }

    fn export(&mut self, _: &ExportKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.cursor_key(cx) else {
            return;
        };
        let items = ExportChoice::ALL
            .iter()
            .map(|choice| PickerItem::new(choice.label()).detail(choice.detail()))
            .collect();
        let picker =
            cx.new(|cx| Picker::new(format!("Export {} as", key.title()), items, window, cx));
        let subscription = cx.subscribe_in(&picker, window, Self::picker_event);
        self.dialog = Some((
            Dialog::Picker(picker, Choosing::Export(key.id)),
            vec![subscription],
        ));
        cx.notify();
    }

    /// Exports key `id` as `choice`. A PKCS#8 export of an encrypted key needs its passphrase:
    /// `typed`, else the saved one, else a remembered one, else the user is asked.
    fn export_as(
        &mut self,
        id: Id<Key>,
        choice: ExportChoice,
        typed: Option<SecretString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let Some(key) = model.key(id).cloned() else {
            return;
        };
        let name = export_file_name(&key, choice);
        if choice == ExportChoice::PublicKey {
            let contents = SecretString::from(format!("{}\n", key.public_key));
            self.save_to_file(&name, contents, false, window, cx);
            return;
        }
        let Some(vault) = model.vault().cloned() else {
            let problem = model.vault_problem().unwrap_or_default();
            notify::info(format!("The key can't be exported: {problem}"), cx);
            return;
        };
        let opened = vault.open_string(&key.private_key).and_then(|private| {
            let saved = match &key.protection {
                KeyProtection::SavedPassphrase(sealed) => Some(vault.open_string(sealed)?),
                KeyProtection::Unencrypted | KeyProtection::AskPassphrase => None,
            };
            Ok((private, saved))
        });
        let (private, saved) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                notify::error(&error, cx);
                return;
            }
        };
        let passphrase = typed.or(saved).or_else(|| {
            cx.try_global::<Passphrases>()
                .and_then(|p| p.0.get(&key.fingerprint))
        });
        if choice == ExportChoice::Pkcs8 && key.protection.is_encrypted() && passphrase.is_none() {
            self.ask_passphrase(&key, window, cx);
            return;
        }
        let format = match choice {
            ExportChoice::Pkcs8 => ExportFormat::Pkcs8,
            ExportChoice::OpenSsh | ExportChoice::PublicKey => ExportFormat::OpenSsh,
        };
        cx.spawn_in(window, async move |this, cx| {
            let exported = cx
                .background_spawn(async move {
                    keys::export_blocking(&private, passphrase.as_ref(), format)
                })
                .await;
            let _ = this.update_in(cx, |list, window, cx| match exported {
                Ok(contents) => list.save_to_file(&name, contents, true, window, cx),
                Err(error) => notify::error(&error, cx),
            });
        })
        .detach();
    }

    fn ask_passphrase(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("", InputKind::Persistent, window, cx).secret());
        let field = input.clone();
        let modal = cx.new(|cx| {
            Modal::new(
                "Key passphrase",
                format!(
                    "Enter the passphrase of {} to export it as PKCS#8.",
                    key.title()
                ),
                "Export",
                window,
                cx,
            )
            .detail(move |_, _| field.clone().into_any_element())
        });
        input.focus_handle(cx).focus(window);
        let id = key.id;
        let subscriptions = vec![
            cx.subscribe_in(
                &modal,
                window,
                move |list, _, event: &ModalEvent, window, cx| {
                    list.passphrase_answered(*event == ModalEvent::Confirmed, id, window, cx);
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                move |list, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed(_) => {}
                    TextInputEvent::Confirmed(_) => list.passphrase_answered(true, id, window, cx),
                    TextInputEvent::Cancelled => list.passphrase_answered(false, id, window, cx),
                },
            ),
        ];
        self.dialog = Some((
            Dialog::Passphrase {
                modal,
                input,
                key: id,
            },
            subscriptions,
        ));
        cx.notify();
    }

    fn passphrase_answered(
        &mut self,
        confirmed: bool,
        id: Id<Key>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((Dialog::Passphrase { input, key, .. }, _)) = self.dialog.take() else {
            return;
        };
        let passphrase = input.update(cx, TextInput::take_secret);
        self.close_dialog(window, cx);
        if confirmed && key == id {
            self.export_as(id, ExportChoice::Pkcs8, Some(passphrase), window, cx);
        }
    }

    /// Asks where to save `contents` (suggesting `name` in the home folder) and writes it there:
    /// readable only by the user if `is_private`.
    #[allow(
        clippy::unused_self,
        reason = "kept a method so every export step reads alike"
    )]
    fn save_to_file(
        &mut self,
        name: &str,
        contents: SecretString,
        is_private: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = cx
            .try_global::<SshEnvironment>()
            .and_then(|env| {
                env.ssh_dir
                    .as_deref()
                    .and_then(|d| d.parent().map(PathBuf::from))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        let chosen = cx.prompt_for_new_path(&dir, Some(name));
        cx.spawn_in(window, async move |_, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let _ = cx.update(|_, cx| {
                        notify::info(format!("Could not open a save dialog: {error}"), cx);
                    });
                    return;
                }
            };
            let target = path.clone();
            let written = cx
                .background_spawn(async move { write_file(&target, &contents, is_private) })
                .await;
            let _ = cx.update(|_, cx| match written {
                Ok(()) => notify::info(format!("Saved {}", path.display()), cx),
                Err(error) => {
                    notify::info(format!("Could not save {}: {error}", path.display()), cx);
                }
            });
        })
        .detach();
    }

    // ── deleting ───────────────────────────────────────────────────────────────────────

    fn delete(&mut self, _: &DeleteKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.cursor_key(cx) else {
            return;
        };
        let used = self.usage.get(&key.id).map_or(0, |u| u.hosts.len());
        let message = match used {
            0 => "This can't be undone.".to_owned(),
            1 => "1 host logs in with it and will no longer. This can't be undone.".to_owned(),
            n => format!("{n} hosts log in with it and will no longer. This can't be undone."),
        };
        let modal = cx.new(|cx| {
            Modal::new(
                format!("Delete {}?", key.title()),
                message,
                "Delete",
                window,
                cx,
            )
            .danger()
        });
        let id = key.id;
        let subscription = cx.subscribe_in(
            &modal,
            window,
            move |list, _, event: &ModalEvent, window, cx| {
                list.close_dialog(window, cx);
                if *event == ModalEvent::Confirmed {
                    list.model.update(cx, |model, cx| model.delete_key(id, cx));
                }
            },
        );
        self.dialog = Some((Dialog::Delete(modal), vec![subscription]));
        cx.notify();
    }
}

/// Writes `contents` to `path`, replacing it; private files are created readable by the user
/// only.
fn write_file(
    path: &std::path::Path,
    contents: &SecretString,
    is_private: bool,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if is_private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = is_private;
    let mut file = options.open(path)?;
    let bytes = Zeroizing::new(contents.expose_secret().as_bytes().to_vec());
    file.write_all(&bytes)?;
    file.sync_all()
}

impl Render for KeyList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let now = Timestamp::now();
        let is_focused = self.focus_handle.contains_focused(window, cx);
        let model = self.model.read(cx);
        let keys: Vec<Key> = sorted(model.keys()).into_iter().cloned().collect();
        let problem = model.vault_problem();

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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Keys"))
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(keys.len().to_string()),
                    ),
            )
            .child(
                Button::new("generate-key", "Generate key")
                    .style(ButtonStyle::Primary)
                    .on_click(cx.listener(|list, _, window, cx| {
                        list.generate(&GenerateKey, window, cx);
                    })),
            )
            .child(
                Button::new("import-key", "Import key").on_click(
                    cx.listener(|list, _, window, cx| list.import(&ImportKey, window, cx)),
                ),
            )
            .child(
                Button::new("import-ssh-dir", "From ~/.ssh").on_click(cx.listener(
                    |list, _, window, cx| list.import_from_ssh_dir(&ImportFromSshDir, window, cx),
                )),
            );

        let rows = keys.iter().enumerate().map(|(index, key)| {
            let is_cursor = self.cursor == Some(key.id);
            let hosts = self.usage.get(&key.id).map_or(0, |u| u.hosts.len());
            let id = key.id;
            let hover = theme.colors.control_hover;
            let mut detail = format!("{} \u{00b7} {}", key.kind.display_name(), key.fingerprint);
            if key.protection.is_encrypted() {
                detail.push_str(" \u{00b7} passphrase");
            }
            div()
                .id(("key", index))
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
                .on_click(
                    cx.listener(move |list, event: &gpui::ClickEvent, window, cx| {
                        list.cursor = Some(id);
                        list.focus_handle.focus(window);
                        if event.click_count() == 2 {
                            list.edit(&EditKey, window, cx);
                        }
                        cx.notify();
                    }),
                )
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
                                .child(key.title().to_owned()),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(theme.text.small_size)
                                .text_color(theme.colors.text_muted)
                                .child(detail),
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
            .id("key-list")
            .key_context("KeyList")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::generate))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::import_from_ssh_dir))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::copy_public_key))
            .on_action(cx.listener(Self::export_to_host))
            .on_action(cx.listener(Self::edit))
            .on_action(cx.listener(Self::delete))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap(theme.spacing.xs)
            .when(keys.is_empty(), |d| d.child(empty_state(&theme)))
            .children(rows);

        let panel = self.cursor_key(cx).map(|key| {
            let model = self.model.read(cx);
            let usage = self.usage.get(&key.id).cloned().unwrap_or_default();
            let title_of_host = |id| model.host(id).map(|h| h.title().into_owned());
            let facts = KeyFacts {
                fingerprints: keys::fingerprints(&key.public_key).ok(),
                hosts: usage
                    .hosts
                    .iter()
                    .filter_map(|id| title_of_host(*id))
                    .collect(),
                identities: usage
                    .identities
                    .iter()
                    .filter_map(|id| model.identity(*id).map(|i| i.title().to_owned()))
                    .collect(),
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
            let mono = mono_family(&theme, window, cx);
            key_panel(
                &key,
                facts,
                &theme,
                &mono,
                now,
                KeyButtons {
                    on_copy: cx.listener(|list, _, window, cx| {
                        list.copy_public_key(&CopyPublicKey, window, cx);
                    }),
                    on_export: cx.listener(|list, _, window, cx| {
                        list.export(&ExportKey, window, cx);
                    }),
                    on_export_to_host: cx.listener(|list, _, window, cx| {
                        list.export_to_host(&ExportKeyToHost, window, cx);
                    }),
                    on_edit: cx.listener(|list, _, window, cx| list.edit(&EditKey, window, cx)),
                    on_delete: cx.listener(|list, _, window, cx| {
                        list.delete(&DeleteKey, window, cx);
                    }),
                },
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
                    .when_some(problem, |d, problem| {
                        d.child(
                            div()
                                .text_color(theme.colors.warning)
                                .child(format!("Saved keys can't be used right now: {problem}")),
                        )
                    })
                    .child(list),
            )
            .children(panel)
    }
}

fn empty_state(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.sm)
        .py(theme.spacing.xl)
        .text_color(theme.colors.text_muted)
        .child("No keys yet.")
        .child("Generate a new key, or import one you already use from a file or ~/.ssh.")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::Sealed;
    use ssherlock_core::keychain::KeyKind;

    use super::*;

    fn key(label: &str, kind: KeyKind) -> Key {
        Key {
            id: Id::generate(),
            label: label.into(),
            kind,
            public_key: "ssh-ed25519 AAAA".into(),
            fingerprint: format!("SHA256:{label}"),
            private_key: Sealed::from_bytes(vec![1]),
            protection: KeyProtection::Unencrypted,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn export_names() {
        let cases = [
            (
                "Work laptop",
                KeyKind::Ed25519,
                ExportChoice::PublicKey,
                "Work_laptop.pub",
            ),
            (
                "deploy-key",
                KeyKind::Ed25519,
                ExportChoice::OpenSsh,
                "deploy-key",
            ),
            ("", KeyKind::Ed25519, ExportChoice::OpenSsh, "id_ed25519"),
            (
                "",
                KeyKind::Rsa(4096),
                ExportChoice::Pkcs8,
                "id_rsa_4096.pem",
            ),
            ("../..", KeyKind::Ed25519, ExportChoice::OpenSsh, ".._.."),
            (
                "///",
                KeyKind::Ed25519,
                ExportChoice::PublicKey,
                "id_ed25519.pub",
            ),
        ];
        for (label, kind, choice, expected) in cases {
            assert_eq!(
                export_file_name(&key(label, kind), choice),
                expected,
                "{label:?}"
            );
        }
    }

    #[test]
    fn short_fingerprints() {
        assert_eq!(
            short_fingerprint("SHA256:iEssdgZzVEZ0tRYtUlI1D5UFmPQON21E7zMspQmTgqo"),
            "SHA256:iEssdgZzVEZ0\u{2026}"
        );
        assert_eq!(short_fingerprint("SHA256:abc"), "SHA256:abc");
    }

    #[test]
    fn keys_sort_by_title_ignoring_case() {
        let keys = [
            key("beta", KeyKind::Ed25519),
            key("Alpha", KeyKind::Ed25519),
            key("", KeyKind::Ed25519),
        ];
        let titles: Vec<&str> = sorted(&keys).into_iter().map(Key::title).collect();
        assert_eq!(titles, ["Alpha", "beta", "SHA256:"]);
    }
}
