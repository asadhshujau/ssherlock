//! [`KeyEditor`]: the form that generates a key (KEY-04), imports one (KEY-05), or edits a saved
//! key's name and whether its passphrase is saved.
//!
//! Slow work (generating RSA keys, decrypting with a passphrase) runs on a background thread
//! while the form says what it's doing. Secrets typed here go straight into the vault; the
//! secret fields never put their text in an event and are wiped when the form closes.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Task, Window, div,
};
use ssherlock_core::keychain::{Key, KeyKind, KeyProtection};
use ssherlock_core::{ExposeSecret as _, Id, SecretString, Timestamp};
use ssherlock_ssh::SshError;
use ssherlock_ssh::keys::{self, KeyMaterial};
use ssherlock_store::Vault;
use zeroize::Zeroizing;

use super::actions::{CancelKeyEditor, SaveKeyEditor};
use crate::components::{
    Button, Checkbox, Form, FormField, FormSection, Segment, SegmentedControl, TextInput, sheet,
};
use crate::hosts::HostsModel;
use crate::notify;
use crate::ssh::status::capitalise;
use crate::theme::ActiveTheme as _;

/// The largest key file the form reads; private keys are a few KB.
const MAX_KEY_FILE: u64 = 64 * 1024;

/// What the form is for.
#[derive(Clone, Debug)]
pub enum KeyEditorMode {
    /// A new key.
    Generate,
    /// A key from a file or the clipboard. `path` pre-fills it from a file (from the `~/.ssh`
    /// list).
    Import {
        /// A file to read the key from straight away.
        path: Option<PathBuf>,
    },
    /// A saved key.
    Edit(Key),
}

/// What the form tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyEditorEvent {
    /// Closed: with the key saved, or `None` if cancelled.
    Done(Option<Id<Key>>),
}

/// The form's fields, for placing error messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    /// The name.
    Label,
    /// The pasted or loaded private key.
    PrivateKey,
    /// The passphrase.
    Passphrase,
    /// The passphrase again.
    Confirm,
    /// Problems that belong to no one field.
    General,
}

/// The key form.
pub struct KeyEditor {
    model: Entity<HostsModel>,
    mode: KeyEditorMode,
    label: Entity<TextInput>,
    comment: Entity<TextInput>,
    private_key: Entity<TextInput>,
    passphrase: Entity<TextInput>,
    confirm: Entity<TextInput>,
    /// Index into [`KeyKind::GENERATE_CHOICES`].
    kind: usize,
    save_passphrase: bool,
    kind_focus: FocusHandle,
    save_focus: FocusHandle,
    errors: HashMap<Field, SharedString>,
    /// What the form is busy doing, and the task doing it.
    busy: Option<(SharedString, Task<()>)>,
    focus_handle: FocusHandle,
}

impl EventEmitter<KeyEditorEvent> for KeyEditor {}

impl Focusable for KeyEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl KeyEditor {
    /// A form for `mode`, with its first field focused.
    pub fn new(
        model: Entity<HostsModel>,
        mode: KeyEditorMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let secret = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextInput::field("", cx).secret().placeholder(placeholder))
        };
        let (label, save_passphrase) = match &mode {
            KeyEditorMode::Edit(key) => (
                key.label.clone(),
                matches!(key.protection, KeyProtection::SavedPassphrase(_)),
            ),
            _ => (String::new(), false),
        };
        let passphrase_placeholder = match &mode {
            KeyEditorMode::Edit(key)
                if matches!(key.protection, KeyProtection::SavedPassphrase(_)) =>
            {
                "Saved; type to replace it"
            }
            KeyEditorMode::Import { .. } => "Only if the key is encrypted",
            _ => "Optional",
        };
        let editor = Self {
            label: cx.new(|cx| TextInput::field(label, cx).placeholder("e.g. Work laptop")),
            comment: cx.new(|cx| TextInput::field("", cx).placeholder("e.g. you@laptop")),
            private_key: cx.new(|cx| {
                TextInput::field("", cx)
                    .secret()
                    .multiline()
                    .placeholder("Paste a private key, or choose a file")
            }),
            passphrase: secret(passphrase_placeholder, cx),
            confirm: secret("The same again", cx),
            model,
            mode,
            kind: 0,
            save_passphrase,
            kind_focus: cx.focus_handle().tab_stop(true),
            save_focus: cx.focus_handle().tab_stop(true),
            errors: HashMap::new(),
            busy: None,
            focus_handle: cx.focus_handle(),
        };
        let first = match &editor.mode {
            KeyEditorMode::Import { .. } => editor.private_key.focus_handle(cx),
            _ => editor.label.focus_handle(cx),
        };
        first.focus(window);
        let mut editor = editor;
        if let KeyEditorMode::Import { path: Some(path) } = editor.mode.clone() {
            editor.load_file(path, cx);
        }
        editor
    }

    /// The title at the top.
    pub fn title(&self) -> String {
        match &self.mode {
            KeyEditorMode::Generate => "Generate a key".to_owned(),
            KeyEditorMode::Import { .. } => "Import a key".to_owned(),
            KeyEditorMode::Edit(key) => format!("Edit {}", key.title()),
        }
    }

    /// The error shown for `field`, if any.
    pub fn error(&self, field: Field) -> Option<&str> {
        self.errors.get(&field).map(SharedString::as_ref)
    }

    /// What the form is busy doing, if anything.
    pub fn busy(&self) -> Option<&str> {
        self.busy.as_ref().map(|(what, _)| what.as_ref())
    }

    fn fail(&mut self, field: Field, message: impl Into<String>) {
        self.errors
            .entry(field)
            .or_insert_with(|| capitalise(&message.into()).into());
    }

    fn vault(&mut self, cx: &App) -> Option<std::sync::Arc<Vault>> {
        let model = self.model.read(cx);
        let vault = model.vault().cloned();
        if vault.is_none() {
            let problem = model
                .vault_problem()
                .map_or_else(String::new, |p| p.to_string());
            self.fail(Field::General, format!("keys can't be saved: {problem}"));
        }
        vault
    }

    // ── loading a key into the import form ─────────────────────────────────────────────

    /// Reads `path` on a background thread into the private key field, and names the key after
    /// the file if it has no name yet.
    fn load_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let read_path = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let read = cx
                .background_spawn(async move {
                    let size = std::fs::metadata(&read_path)?.len();
                    if size > MAX_KEY_FILE {
                        return Err(std::io::Error::other("the file is too large to be a key"));
                    }
                    std::fs::read_to_string(&read_path).map(Zeroizing::new)
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.busy = None;
                match read {
                    Ok(text) => {
                        editor
                            .private_key
                            .update(cx, |input, cx| input.set_text(text.as_str(), cx));
                        if editor.label.read(cx).is_empty()
                            && let Some(name) = path.file_name()
                        {
                            let name = name.to_string_lossy().into_owned();
                            editor
                                .label
                                .update(cx, |input, cx| input.set_text(name, cx));
                        }
                    }
                    Err(error) => editor.fail(
                        Field::PrivateKey,
                        format!("could not read {}: {error}", path.display()),
                    ),
                }
                cx.notify();
            });
        });
        self.busy = Some(("Reading the file\u{2026}".into(), task));
        cx.notify();
    }

    #[allow(
        clippy::unused_self,
        reason = "called from a gpui listener, which passes `&mut self`"
    )]
    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |editor, cx| {
                        editor.fail(
                            Field::PrivateKey,
                            format!("could not open a file chooser: {error}"),
                        );
                        cx.notify();
                    });
                    None
                }
            };
            if let Some(path) = chosen {
                let _ = this.update(cx, |editor, cx| editor.load_file(path, cx));
            }
        })
        .detach();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        match text {
            Some(text) => {
                let text = Zeroizing::new(text);
                self.private_key
                    .update(cx, |input, cx| input.set_text(text.as_str(), cx));
                self.errors.remove(&Field::PrivateKey);
            }
            None => self.fail(Field::PrivateKey, "the clipboard has no text"),
        }
        cx.notify();
    }

    // ── saving ─────────────────────────────────────────────────────────────────────────

    fn save(&mut self, _: &SaveKeyEditor, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.errors.clear();
        match self.mode.clone() {
            KeyEditorMode::Generate => self.save_generated(cx),
            KeyEditorMode::Import { .. } => self.save_imported(cx),
            KeyEditorMode::Edit(key) => self.save_edited(&key, cx),
        }
        cx.notify();
    }

    fn text(input: &Entity<TextInput>, cx: &App) -> String {
        input.read(cx).text().trim().to_owned()
    }

    /// The typed passphrase, or `None` if the field is empty.
    fn typed_passphrase(&self, cx: &App) -> Option<SecretString> {
        let input = self.passphrase.read(cx);
        (!input.is_empty()).then(|| input.secret_text())
    }

    fn save_generated(&mut self, cx: &mut Context<Self>) {
        let Some(vault) = self.vault(cx) else { return };
        let passphrase = self.typed_passphrase(cx);
        let confirm = self.confirm.read(cx).secret_text();
        let matches = passphrase
            .as_ref()
            .map_or(confirm.expose_secret().is_empty(), |p| {
                p.expose_secret() == confirm.expose_secret()
            });
        if !matches {
            self.fail(Field::Confirm, "the passphrases don't match");
            return;
        }
        let kind = KeyKind::GENERATE_CHOICES
            .get(self.kind)
            .copied()
            .unwrap_or(KeyKind::Ed25519);
        let comment = Self::text(&self.comment, cx);
        let label = Self::text(&self.label, cx);
        let label = if label.is_empty() {
            comment.clone()
        } else {
            label
        };
        let save_passphrase = self.save_passphrase;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    keys::generate_blocking(kind, &comment, passphrase.as_ref())
                        .map(|material| (material, passphrase))
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.busy = None;
                editor.finish_new(result, label, save_passphrase, &vault, cx);
            });
        });
        let what = match kind {
            KeyKind::Rsa(_) => "Generating the key; RSA takes a few seconds\u{2026}",
            _ => "Generating the key\u{2026}",
        };
        self.busy = Some((what.into(), task));
    }

    fn save_imported(&mut self, cx: &mut Context<Self>) {
        let Some(vault) = self.vault(cx) else { return };
        if self.private_key.read(cx).is_empty() {
            self.fail(Field::PrivateKey, "paste a private key or choose a file");
            return;
        }
        let text = self.private_key.read(cx).secret_text();
        let passphrase = self.typed_passphrase(cx);
        let label = Self::text(&self.label, cx);
        let save_passphrase = self.save_passphrase;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    keys::import_blocking(&text, passphrase.as_ref())
                        .map(|material| (material, passphrase))
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.busy = None;
                editor.finish_new(result, label, save_passphrase, &vault, cx);
            });
        });
        self.busy = Some(("Reading the key\u{2026}".into(), task));
    }

    /// Seals and saves a generated or imported key, or shows why it can't be.
    fn finish_new(
        &mut self,
        result: Result<(KeyMaterial, Option<SecretString>), SshError>,
        label: String,
        save_passphrase: bool,
        vault: &Vault,
        cx: &mut Context<Self>,
    ) {
        let (material, passphrase) = match result {
            Ok(done) => done,
            Err(error) => {
                self.key_error(&error);
                cx.notify();
                return;
            }
        };
        if let Some(existing) = self
            .model
            .read(cx)
            .keys()
            .iter()
            .find(|k| k.fingerprint == material.fingerprint)
        {
            let title = existing.title().to_owned();
            self.fail(
                Field::PrivateKey,
                format!("this key is already in the keychain as {title}"),
            );
            cx.notify();
            return;
        }
        let built = build_key(
            material,
            label,
            passphrase.filter(|_| save_passphrase),
            vault,
        );
        match built {
            Ok(key) => {
                if let Err(error) = key.validate() {
                    self.fail(Field::Label, error.to_string());
                    cx.notify();
                    return;
                }
                let id = key.id;
                self.model.update(cx, |model, cx| model.save_key(key, cx));
                cx.emit(KeyEditorEvent::Done(Some(id)));
            }
            Err(error) => {
                notify::error(&error, cx);
                self.fail(Field::General, error.to_string());
            }
        }
        cx.notify();
    }

    /// Puts a key error under the field it's about.
    fn key_error(&mut self, error: &SshError) {
        match error {
            SshError::KeyNeedsPassphrase => {
                self.fail(
                    Field::Passphrase,
                    "this key is encrypted; enter its passphrase",
                );
            }
            SshError::KeyWrongPassphrase => self.fail(Field::Passphrase, "the passphrase is wrong"),
            SshError::KeyUnreadable { .. } | SshError::KeyUnsupported { .. } => {
                self.fail(Field::PrivateKey, error.to_string());
            }
            _ => self.fail(Field::General, error.to_string()),
        }
    }

    fn save_edited(&mut self, key: &Key, cx: &mut Context<Self>) {
        let label = Self::text(&self.label, cx);
        let typed = self.typed_passphrase(cx);
        let keep = |protection: KeyProtection| Key {
            label: label.clone(),
            protection,
            updated_at: Timestamp::now(),
            ..key.clone()
        };
        let edited = match (&key.protection, self.save_passphrase, typed) {
            (KeyProtection::Unencrypted, _, _) => keep(KeyProtection::Unencrypted),
            (_, false, _) => keep(KeyProtection::AskPassphrase),
            (KeyProtection::SavedPassphrase(saved), true, None) => {
                keep(KeyProtection::SavedPassphrase(saved.clone()))
            }
            (KeyProtection::AskPassphrase, true, None) => {
                self.fail(Field::Passphrase, "enter the passphrase to save it");
                return;
            }
            (_, true, Some(passphrase)) => {
                // Check the passphrase opens the key before saving it.
                let Some(vault) = self.vault(cx) else { return };
                let private = match vault.open_string(&key.private_key) {
                    Ok(private) => private,
                    Err(error) => {
                        self.fail(Field::General, error.to_string());
                        return;
                    }
                };
                let base = keep(KeyProtection::AskPassphrase);
                let task = cx.spawn(async move |this, cx| {
                    let checked = cx
                        .background_spawn(async move {
                            keys::open_blocking(&private, Some(&passphrase)).map(|_| passphrase)
                        })
                        .await;
                    let _ = this.update(cx, |editor, cx| {
                        editor.busy = None;
                        match checked.map_err(|e| (e, None)).and_then(|passphrase| {
                            vault.seal_str(&passphrase).map_err(|e| {
                                (
                                    SshError::KeyExport {
                                        reason: e.to_string(),
                                    },
                                    Some(()),
                                )
                            })
                        }) {
                            Ok(sealed) => editor.finish_edit(
                                Key {
                                    protection: KeyProtection::SavedPassphrase(sealed),
                                    ..base
                                },
                                cx,
                            ),
                            Err((error, _)) => {
                                editor.key_error(&error);
                                cx.notify();
                            }
                        }
                    });
                });
                self.busy = Some(("Checking the passphrase\u{2026}".into(), task));
                return;
            }
        };
        self.finish_edit(edited, cx);
    }

    fn finish_edit(&mut self, key: Key, cx: &mut Context<Self>) {
        if let Err(error) = key.validate() {
            self.fail(Field::Label, error.to_string());
            cx.notify();
            return;
        }
        let id = key.id;
        self.model.update(cx, |model, cx| model.save_key(key, cx));
        cx.emit(KeyEditorEvent::Done(Some(id)));
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn cancel(&mut self, _: &CancelKeyEditor, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(KeyEditorEvent::Done(None));
    }
}

/// A [`Key`] from `material`, its private half sealed, and `passphrase` sealed too if the
/// user chose to save it.
fn build_key(
    material: KeyMaterial,
    label: String,
    passphrase: Option<SecretString>,
    vault: &Vault,
) -> Result<Key, ssherlock_store::StoreError> {
    let now = Timestamp::now();
    let protection = match (material.is_encrypted, passphrase) {
        (false, _) => KeyProtection::Unencrypted,
        (true, Some(passphrase)) => KeyProtection::SavedPassphrase(vault.seal_str(&passphrase)?),
        (true, None) => KeyProtection::AskPassphrase,
    };
    Ok(Key {
        id: Id::generate(),
        label,
        kind: material.kind,
        public_key: material.public_key,
        fingerprint: material.fingerprint,
        private_key: vault.seal_str(&material.private_key)?,
        protection,
        created_at: now,
        updated_at: now,
    })
}

impl Render for KeyEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error = |field: Field| self.errors.get(&field).cloned();
        let save_label = match &self.mode {
            KeyEditorMode::Generate => "Generate",
            KeyEditorMode::Import { .. } => "Import",
            KeyEditorMode::Edit(_) => "Save",
        };
        let is_encrypted_key = match &self.mode {
            KeyEditorMode::Edit(key) => key.protection.is_encrypted(),
            _ => true,
        };
        let save_checkbox = Checkbox::new(
            "save-passphrase",
            "Save the passphrase in the vault",
            self.save_passphrase,
            &self.save_focus,
        )
        .on_toggle(cx.listener(|editor, value: &bool, _, cx| {
            editor.save_passphrase = *value;
            cx.notify();
        }));
        let save_hint = "Otherwise SSHerlock asks for it when the key is used, and remembers it \
                         for 15 minutes";
        let label_field = FormField::new("Name", self.label.clone()).error(error(Field::Label));

        let form = match &self.mode {
            KeyEditorMode::Generate => {
                let segments = KeyKind::GENERATE_CHOICES
                    .iter()
                    .map(|kind| Segment::new(kind_label(*kind)))
                    .collect();
                Form::new()
                    .child(
                        FormSection::new("Key")
                            .child(label_field)
                            .child(
                                FormField::new(
                                    "Type",
                                    SegmentedControl::new(
                                        "kind",
                                        segments,
                                        self.kind,
                                        &self.kind_focus,
                                    )
                                    .on_select(cx.listener(
                                        |editor, index: &usize, _, cx| {
                                            editor.kind = *index;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .hint(
                                    "Ed25519 is short, fast and accepted by every current server; P-256 to P-521 are ECDSA",
                                ),
                            )
                            .child(
                                FormField::new("Comment", self.comment.clone())
                                    .hint("Added to the end of the public key"),
                            ),
                    )
                    .child(
                        FormSection::new("Passphrase")
                            .child(
                                FormField::new("Passphrase", self.passphrase.clone())
                                    .error(error(Field::Passphrase)),
                            )
                            .child(
                                FormField::new("Again", self.confirm.clone())
                                    .error(error(Field::Confirm)),
                            )
                            .child(FormField::new("", save_checkbox).hint(save_hint)),
                    )
            }
            KeyEditorMode::Import { .. } => Form::new()
                .child(
                    FormSection::new("Key")
                        .child(
                            FormField::new("Private key", self.private_key.clone())
                                .error(error(Field::PrivateKey)),
                        )
                        .child(FormField::new(
                            "",
                            div()
                                .flex()
                                .gap(theme.spacing.md)
                                .child(
                                    Button::new("key-paste", "Paste")
                                        .on_click(cx.listener(|editor, _, _, cx| editor.paste(cx))),
                                )
                                .child(Button::new("key-file", "Choose file\u{2026}").on_click(
                                    cx.listener(|editor, _, _, cx| editor.choose_file(cx)),
                                )),
                        ))
                        .child(label_field),
                )
                .child(
                    FormSection::new("Passphrase")
                        .child(
                            FormField::new("Passphrase", self.passphrase.clone())
                                .error(error(Field::Passphrase)),
                        )
                        .child(FormField::new("", save_checkbox).hint(save_hint)),
                ),
            KeyEditorMode::Edit(_) => Form::new()
                .child(FormSection::new("Key").child(label_field))
                .when(is_encrypted_key, |form| {
                    form.child(
                        FormSection::new("Passphrase")
                            .child(FormField::new("", save_checkbox).hint(save_hint))
                            .when(self.save_passphrase, |section| {
                                section.child(
                                    FormField::new("Passphrase", self.passphrase.clone())
                                        .error(error(Field::Passphrase)),
                                )
                            }),
                    )
                }),
        };
        let status = self
            .busy
            .as_ref()
            .map(|(what, _)| (what.clone(), theme.colors.text_muted))
            .or_else(|| error(Field::General).map(|e| (e, theme.colors.danger)));
        let body = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.lg)
            .child(form)
            .when_some(status, |d, (text, colour)| {
                d.child(div().text_color(colour).child(text))
            });

        sheet(
            &theme,
            self.title(),
            save_label,
            body.into_any_element(),
            cx.listener(|editor, _, window, cx| editor.cancel(&CancelKeyEditor, window, cx)),
            cx.listener(|editor, _, window, cx| editor.save(&SaveKeyEditor, window, cx)),
        )
        .key_context("KeyEditor")
        .track_focus(&self.focus_handle)
        .on_action(cx.listener(Self::save))
        .on_action(cx.listener(Self::cancel))
    }
}

/// A short label for the type picker, so the six choices fit on one row.
fn kind_label(kind: KeyKind) -> String {
    match kind {
        KeyKind::Ecdsa(curve) => format!("P-{}", curve.bits()),
        KeyKind::Ed25519 | KeyKind::Rsa(_) => kind.display_name(),
    }
}

/// Copies `text` (a public key) to the clipboard.
pub fn copy_text(text: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui::{Subscription, TestAppContext, VisualTestContext};
    use pretty_assertions::assert_eq;
    use ssherlock_store::Store;

    use super::*;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ssherlock-ssh/tests/fixtures"
    );

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()
    }

    struct Fixture {
        editor: Entity<KeyEditor>,
        model: Entity<HostsModel>,
        events: Rc<RefCell<Vec<KeyEditorEvent>>>,
        _subscription: Subscription,
    }

    fn open(
        mode: KeyEditorMode,
        model: Option<Entity<HostsModel>>,
        cx: &mut TestAppContext,
    ) -> (Fixture, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::keymap::init(cx);
            notify::init(cx);
        });
        let model = model.unwrap_or_else(|| {
            let store = Arc::new(Store::open_in_memory().unwrap());
            cx.update(|cx| HostsModel::install(store, cx))
        });
        cx.run_until_parked();
        let (editor, cx) =
            cx.add_window_view(|window, cx| KeyEditor::new(model.clone(), mode, window, cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let subscription = cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &KeyEditorEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
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

    fn set(
        f: &Fixture,
        field: fn(&KeyEditor) -> &Entity<TextInput>,
        text: &str,
        cx: &mut VisualTestContext,
    ) {
        let input = f.editor.read_with(cx, |editor, _| field(editor).clone());
        input.update(cx, |input, cx| input.set_text(text, cx));
    }

    fn save(f: &Fixture, cx: &mut VisualTestContext) {
        f.editor.update_in(cx, |editor, window, cx| {
            editor.save(&SaveKeyEditor, window, cx);
        });
        cx.run_until_parked();
    }

    fn error(f: &Fixture, field: Field, cx: &mut VisualTestContext) -> Option<String> {
        f.editor
            .read_with(cx, |editor, _| editor.error(field).map(str::to_owned))
    }

    fn keys(f: &Fixture, cx: &mut VisualTestContext) -> Vec<Key> {
        f.model.read_with(cx, |m, _| m.keys().to_vec())
    }

    fn vault(f: &Fixture, cx: &mut VisualTestContext) -> Arc<Vault> {
        f.model.read_with(cx, |m, _| m.vault().cloned()).unwrap()
    }

    #[gpui::test]
    fn generating_types_a_name_and_saves_a_sealed_key(cx: &mut TestAppContext) {
        let (f, cx) = open(KeyEditorMode::Generate, None, cx);
        cx.simulate_input("Laptop");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        let saved = keys(&f, cx);
        assert_eq!(saved.len(), 1);
        let key = saved.first().unwrap();
        assert_eq!(key.label, "Laptop");
        assert_eq!(key.kind, KeyKind::Ed25519);
        assert_eq!(key.protection, KeyProtection::Unencrypted);
        assert_eq!(f.events.take(), [KeyEditorEvent::Done(Some(key.id))]);
        // The sealed private key opens to a key with the same fingerprint.
        let private = vault(&f, cx).open_string(&key.private_key).unwrap();
        let back = keys::import_blocking(&private, None).unwrap();
        assert_eq!(back.fingerprint, key.fingerprint);
    }

    #[gpui::test]
    fn generating_with_a_passphrase_checks_it_twice_and_can_save_it(cx: &mut TestAppContext) {
        let (f, cx) = open(KeyEditorMode::Generate, None, cx);
        set(&f, |e| &e.passphrase, "one", cx);
        set(&f, |e| &e.confirm, "two", cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::Confirm, cx).as_deref(),
            Some("The passphrases don't match")
        );
        assert_eq!(keys(&f, cx).len(), 0);

        set(&f, |e| &e.confirm, "one", cx);
        f.editor
            .update(cx, |editor, _| editor.save_passphrase = true);
        save(&f, cx);
        let key = keys(&f, cx).pop().unwrap();
        let KeyProtection::SavedPassphrase(sealed) = &key.protection else {
            panic!("expected a saved passphrase, got {:?}", key.protection);
        };
        let vault = vault(&f, cx);
        assert_eq!(vault.open_string(sealed).unwrap().expose_secret(), "one");
        let private = vault.open_string(&key.private_key).unwrap();
        assert!(keys::is_encrypted(&private).unwrap());
    }

    #[gpui::test]
    fn importing_reports_what_is_wrong_and_refuses_duplicates(cx: &mut TestAppContext) {
        let (f, cx) = open(KeyEditorMode::Import { path: None }, None, cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::PrivateKey, cx).as_deref(),
            Some("Paste a private key or choose a file")
        );

        // Encrypted PEM needs its passphrase to be read at all.
        set(&f, |e| &e.private_key, &fixture("id_rsa_pem_encrypted"), cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::Passphrase, cx).as_deref(),
            Some("This key is encrypted; enter its passphrase")
        );
        set(&f, |e| &e.passphrase, "nope", cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::Passphrase, cx).as_deref(),
            Some("The passphrase is wrong")
        );
        set(&f, |e| &e.passphrase, "fixture-passphrase", cx);
        save(&f, cx);
        let key = keys(&f, cx).pop().unwrap();
        assert_eq!(key.kind, KeyKind::Rsa(3072));
        assert_eq!(key.protection, KeyProtection::AskPassphrase);
        assert_eq!(f.events.take(), [KeyEditorEvent::Done(Some(key.id))]);

        // The same key again (here as PKCS#8) is refused.
        set(&f, |e| &e.private_key, &fixture("id_rsa_pkcs8"), cx);
        set(&f, |e| &e.passphrase, "", cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::PrivateKey, cx),
            Some(format!(
                "This key is already in the keychain as {}",
                key.title()
            ))
        );
        assert_eq!(keys(&f, cx).len(), 1);
    }

    #[gpui::test]
    fn editing_saves_a_checked_passphrase_and_can_forget_it(cx: &mut TestAppContext) {
        let (f, cx) = open(KeyEditorMode::Import { path: None }, None, cx);
        set(&f, |e| &e.private_key, &fixture("id_ed25519_encrypted"), cx);
        set(&f, |e| &e.label, "Encrypted", cx);
        save(&f, cx);
        let key = keys(&f, cx).pop().unwrap();
        assert_eq!(key.protection, KeyProtection::AskPassphrase);
        let model = f.model.clone();
        drop(f);

        let (f, cx) = open(KeyEditorMode::Edit(key.clone()), Some(model), cx);
        f.editor
            .update(cx, |editor, _| editor.save_passphrase = true);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::Passphrase, cx).as_deref(),
            Some("Enter the passphrase to save it")
        );
        set(&f, |e| &e.passphrase, "wrong", cx);
        save(&f, cx);
        assert_eq!(
            error(&f, Field::Passphrase, cx).as_deref(),
            Some("The passphrase is wrong")
        );
        set(&f, |e| &e.passphrase, "fixture-passphrase", cx);
        set(&f, |e| &e.label, "Renamed", cx);
        save(&f, cx);
        let edited = keys(&f, cx).pop().unwrap();
        assert_eq!(edited.label, "Renamed");
        assert!(matches!(
            edited.protection,
            KeyProtection::SavedPassphrase(_)
        ));
        assert_eq!(
            edited.private_key, key.private_key,
            "the key itself is untouched"
        );
    }
}
