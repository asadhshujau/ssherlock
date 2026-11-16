//! [`HostsModel`]: the saved hosts, groups, trash, keys and identities, shared by every view
//! that shows them.
//!
//! One per app. It owns the [`Store`] and keeps an in-memory copy of what's in it, so views
//! render from memory and never touch the database. Every change goes to the store on a
//! background thread; changes run one at a time, in the order they were asked for, and each
//! ends by reloading, so the copy always matches the database. Failures become toasts.
//!
//! It also unlocks the [`Vault`] once the database is open, so views can seal secrets before
//! saving them and connections can open them. Keys and identities live here rather than in a
//! model of their own because hosts need identities to resolve, and the keychain needs hosts to
//! say what uses a key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use ssherlock_core::hosts::{Group, Host};
use ssherlock_core::keychain::{Identity, Key};
use ssherlock_core::trash::TrashEntry;
use ssherlock_core::{Id, Timestamp};
use ssherlock_store::vault::MemoryKeyHolder;
use ssherlock_store::{KeyFile, KeyLocation, OsKeychain, Repository, Store, StoreError, Vault};

use crate::notify;

/// Whether the hosts have been loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// The database is being opened.
    Loading,
    /// The data is in memory.
    Ready,
    /// The database couldn't be opened; the message says why.
    Failed(SharedString),
}

/// Whether saved secrets can be sealed and opened.
#[derive(Clone, Debug)]
pub enum VaultState {
    /// Waiting for the database, or for the OS keychain (which may be asking the user).
    Unlocking,
    /// Ready.
    Ready(Arc<Vault>),
    /// It couldn't be unlocked; the message says why. Hosts still work; saved passwords and
    /// keys don't.
    Unavailable(SharedString),
}

/// Everything the hosts views show, read in one go.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    hosts: Vec<Host>,
    groups: Vec<Group>,
    trash: Vec<TrashEntry>,
    keys: Vec<Key>,
    identities: Vec<Identity>,
}

impl Snapshot {
    fn load(store: &Store) -> Result<Self, StoreError> {
        let mut trash: Vec<TrashEntry> = store.list()?;
        trash.sort_by_key(|entry| std::cmp::Reverse(entry.deleted_at));
        Ok(Self {
            hosts: store.list()?,
            groups: store.list()?,
            trash,
            keys: store.list()?,
            identities: store.list()?,
        })
    }
}

/// The saved hosts, groups and trash, and the way to change them.
pub struct HostsModel {
    store: Option<Arc<Store>>,
    state: LoadState,
    vault: VaultState,
    data: Snapshot,
    /// The last queued store operation; the next one waits for it.
    queue: Option<Task<()>>,
    /// Tabs open to each host right now (HOST-12). Not saved.
    sessions: HashMap<Id<Host>, usize>,
}

struct GlobalHostsModel(Entity<HostsModel>);

impl Global for GlobalHostsModel {}

impl HostsModel {
    /// The app's model, if [`open`](Self::open) or [`install`](Self::install) has run.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalHostsModel>().map(|g| g.0.clone())
    }

    /// Opens the database at `path` in the background and installs the model as the app's,
    /// then unlocks the vault, keeping its key in the OS keychain or else in `key_file`.
    pub fn open(path: PathBuf, key_file: PathBuf, cx: &mut App) -> Entity<Self> {
        let model = cx.new(|cx| {
            let mut model = Self::empty(LoadState::Loading);
            model.queue = Some(cx.spawn(async move |this, cx| {
                let opened = cx
                    .background_spawn(async move {
                        let store = Arc::new(Store::open(&path)?);
                        let snapshot = Snapshot::load(&store)?;
                        Ok::<_, StoreError>((store, snapshot))
                    })
                    .await;
                let store = this
                    .update(cx, |model: &mut Self, cx| {
                        let store = match opened {
                            Ok((store, snapshot)) => {
                                model.store = Some(store.clone());
                                model.data = snapshot;
                                model.state = LoadState::Ready;
                                Some(store)
                            }
                            Err(error) => {
                                notify::error(&error, cx);
                                model.state = LoadState::Failed(error_text(&error).into());
                                model.vault = VaultState::Unavailable(
                                    "the database could not be opened".into(),
                                );
                                None
                            }
                        };
                        cx.notify();
                        store
                    })
                    .ok()
                    .flatten();
                let Some(store) = store else { return };
                // Separately from loading, so hosts show while the keychain asks for a password.
                let unlocked = cx
                    .background_spawn(async move {
                        store.unlock_vault(&OsKeychain, &KeyFile::new(key_file))
                    })
                    .await;
                let _ = this.update(cx, |model: &mut Self, cx| {
                    model.vault_unlocked(unlocked, cx);
                });
            }));
            model
        });
        cx.set_global(GlobalHostsModel(model.clone()));
        model
    }

    /// Installs a model over an already-open `store`, for tests. Its vault key is kept in
    /// memory only, so the real keychain is never touched.
    pub fn install(store: Arc<Store>, cx: &mut App) -> Entity<Self> {
        let model = cx.new(|cx| {
            let mut model = Self::empty(LoadState::Loading);
            let unlocked =
                store.unlock_vault(&MemoryKeyHolder::default(), &MemoryKeyHolder::unavailable());
            model.vault_unlocked(unlocked, cx);
            model.store = Some(store);
            model.reload(cx);
            model
        });
        cx.set_global(GlobalHostsModel(model.clone()));
        model
    }

    /// Installs a model that explains hosts are unavailable, e.g. with no data directory.
    pub fn unavailable(reason: impl Into<SharedString>, cx: &mut App) -> Entity<Self> {
        let model = cx.new(|_| Self::empty(LoadState::Failed(reason.into())));
        cx.set_global(GlobalHostsModel(model.clone()));
        model
    }

    fn vault_unlocked(
        &mut self,
        unlocked: Result<ssherlock_store::Unlocked, StoreError>,
        cx: &mut Context<Self>,
    ) {
        self.vault = match unlocked {
            Ok(unlocked) => {
                // Said once, when the vault is made; every start after uses the same file.
                if unlocked.location == KeyLocation::File && unlocked.is_new {
                    notify::info(
                        "No OS keychain was found, so saved passwords and keys are protected by \
                         a key file in SSHerlock's data folder",
                        cx,
                    );
                }
                VaultState::Ready(Arc::new(unlocked.vault))
            }
            Err(error) => {
                notify::error(&error, cx);
                VaultState::Unavailable(error_text(&error).into())
            }
        };
        cx.notify();
    }

    fn empty(state: LoadState) -> Self {
        Self {
            store: None,
            state,
            vault: VaultState::Unlocking,
            data: Snapshot::default(),
            queue: None,
            sessions: HashMap::new(),
        }
    }

    /// Whether the hosts are loaded, still loading, or unavailable.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Whether saved secrets can be used.
    pub fn vault_state(&self) -> &VaultState {
        &self.vault
    }

    /// The vault, if it's unlocked.
    pub fn vault(&self) -> Option<&Arc<Vault>> {
        match &self.vault {
            VaultState::Ready(vault) => Some(vault),
            VaultState::Unlocking | VaultState::Unavailable(_) => None,
        }
    }

    /// Why secrets can't be used right now, for messages; `None` when they can.
    pub fn vault_problem(&self) -> Option<SharedString> {
        match &self.vault {
            VaultState::Ready(_) => None,
            VaultState::Unlocking => Some("the vault is still unlocking".into()),
            VaultState::Unavailable(reason) => Some(reason.clone()),
        }
    }

    /// The key with `id`, if it's saved.
    pub fn key(&self, id: Id<Key>) -> Option<&Key> {
        self.data.keys.iter().find(|k| k.id == id)
    }

    /// The identity with `id`, if it's saved.
    pub fn identity(&self, id: Id<Identity>) -> Option<&Identity> {
        self.data.identities.iter().find(|i| i.id == id)
    }

    /// Every saved host, in no particular order.
    pub fn hosts(&self) -> &[Host] {
        &self.data.hosts
    }

    /// Every group, in no particular order.
    pub fn groups(&self) -> &[Group] {
        &self.data.groups
    }

    /// The trash, most recently deleted first.
    pub fn trash(&self) -> &[TrashEntry] {
        &self.data.trash
    }

    /// Every saved key, in no particular order.
    pub fn keys(&self) -> &[Key] {
        &self.data.keys
    }

    /// Every identity, in no particular order. Hosts need them to resolve their credentials.
    pub fn identities(&self) -> &[Identity] {
        &self.data.identities
    }

    /// The host with `id`, if it's saved.
    pub fn host(&self, id: Id<Host>) -> Option<&Host> {
        self.data.hosts.iter().find(|h| h.id == id)
    }

    /// How many tabs are open to `id`.
    pub fn open_sessions(&self, id: Id<Host>) -> usize {
        self.sessions.get(&id).copied().unwrap_or(0)
    }

    /// Notes that a tab to `id` opened.
    pub fn session_opened(&mut self, id: Id<Host>, cx: &mut Context<Self>) {
        *self.sessions.entry(id).or_default() += 1;
        cx.notify();
    }

    /// Notes that a tab to `id` closed.
    pub fn session_closed(&mut self, id: Id<Host>, cx: &mut Context<Self>) {
        if let Some(count) = self.sessions.get_mut(&id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.sessions.remove(&id);
            }
            cx.notify();
        }
    }

    /// Saves `hosts` (new or edited), all or nothing.
    pub fn save_hosts(&mut self, hosts: Vec<Host>, cx: &mut Context<Self>) {
        self.run(move |store| store.upsert_all(&hosts), cx);
    }

    /// Saves a new or edited group.
    pub fn save_group(&mut self, group: Group, cx: &mut Context<Self>) {
        self.run(move |store| store.upsert(&group), cx);
    }

    /// Marks hosts as favourites, or not. Reads the hosts from the store when the change runs,
    /// so it builds on every change queued before it.
    pub fn set_favourite(&mut self, ids: &[Id<Host>], is_favourite: bool, cx: &mut Context<Self>) {
        let ids = ids.to_vec();
        self.run(
            move |store| {
                let now = Timestamp::now();
                let mut changed = Vec::new();
                for id in ids {
                    if let Some(host) = Repository::<Host>::get(store, id)?
                        && host.is_favourite != is_favourite
                    {
                        changed.push(Host {
                            is_favourite,
                            updated_at: now,
                            ..host
                        });
                    }
                }
                store.upsert_all(&changed)
            },
            cx,
        );
    }

    /// Moves hosts to the trash, one entry each.
    pub fn trash_hosts(&mut self, ids: Vec<Id<Host>>, cx: &mut Context<Self>) {
        let now = Timestamp::now();
        self.run(move |store| store.trash_hosts(&ids, now).map(drop), cx);
    }

    /// Moves a group, with everything inside it, to the trash.
    pub fn trash_group(&mut self, id: Id<Group>, cx: &mut Context<Self>) {
        let now = Timestamp::now();
        self.run(move |store| store.trash_group(id, now).map(drop), cx);
    }

    /// Puts a trash entry's contents back.
    pub fn restore(&mut self, id: Id<TrashEntry>, cx: &mut Context<Self>) {
        self.run(move |store| store.restore(id), cx);
    }

    /// Deletes a trash entry for good.
    pub fn delete_forever(&mut self, id: Id<TrashEntry>, cx: &mut Context<Self>) {
        self.run(move |store| Repository::<TrashEntry>::delete(store, id), cx);
    }

    /// Deletes everything in the trash for good.
    pub fn empty_trash(&mut self, cx: &mut Context<Self>) {
        self.run(Store::empty_trash, cx);
    }

    /// Saves a new or edited key. Its secrets must already be sealed with [`vault`](Self::vault).
    pub fn save_key(&mut self, key: Key, cx: &mut Context<Self>) {
        self.run(move |store| store.upsert(&key), cx);
    }

    /// Deletes a key for good, and stops every host, group and identity from using it.
    pub fn delete_key(&mut self, id: Id<Key>, cx: &mut Context<Self>) {
        self.run(move |store| Repository::<Key>::delete(store, id), cx);
    }

    /// Saves a new or edited identity. Its password must already be sealed.
    pub fn save_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.run(move |store| store.upsert(&identity), cx);
    }

    /// Deletes an identity for good, and stops every host and group from using it.
    pub fn delete_identity(&mut self, id: Id<Identity>, cx: &mut Context<Self>) {
        self.run(move |store| Repository::<Identity>::delete(store, id), cx);
    }

    /// Notes a connection to `id` now, for Recents.
    pub fn record_connection(&mut self, id: Id<Host>, cx: &mut Context<Self>) {
        let now = Timestamp::now();
        self.run(move |store| store.record_connection(id, now), cx);
    }

    /// Reloads everything from the store.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.run(|_| Ok(()), cx);
    }

    /// Queues `op` on the store, then reloads. Waits for earlier operations, so changes land in
    /// the order they were made.
    fn run(
        &mut self,
        op: impl FnOnce(&Store) -> Result<(), StoreError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store.clone() else {
            tracing::warn!("a hosts change was ignored because the database isn't open");
            return;
        };
        let previous = self.queue.take();
        self.queue = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_spawn(async move {
                    let outcome = op(&store);
                    // Reload even after a failure: part of a batch may be committed elsewhere,
                    // and the copy must match the database either way.
                    (outcome, Snapshot::load(&store))
                })
                .await;
            let _ = this.update(cx, |model, cx| {
                let (outcome, snapshot) = result;
                if let Err(error) = outcome {
                    notify::error(&error, cx);
                }
                match snapshot {
                    Ok(snapshot) => {
                        model.data = snapshot;
                        model.state = LoadState::Ready;
                    }
                    Err(error) => notify::error(&error, cx),
                }
                cx.notify();
            });
        }));
    }

    /// Waits for every queued operation; for tests.
    #[cfg(test)]
    pub fn flush(&mut self) -> Option<Task<()>> {
        self.queue.take()
    }
}

/// `error` with its causes, for showing where the hosts list would be.
fn error_text(error: &StoreError) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;

    use super::*;

    fn model(cx: &mut TestAppContext) -> Entity<HostsModel> {
        cx.update(notify::init);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let model = cx.update(|cx| HostsModel::install(store, cx));
        cx.run_until_parked();
        model
    }

    fn host(address: &str) -> Host {
        Host::new(address.parse().unwrap(), Timestamp::UNIX_EPOCH)
    }

    #[gpui::test]
    fn changes_reach_the_store_in_order_and_reload(cx: &mut TestAppContext) {
        let model = model(cx);
        assert_eq!(
            model.read_with(cx, |m, _| m.state().clone()),
            LoadState::Ready
        );
        let a = host("a.example.com");
        let b = host("b.example.com");
        model.update(cx, |m, cx| {
            m.save_hosts(vec![a.clone(), b.clone()], cx);
            m.set_favourite(&[a.id], true, cx);
            m.trash_hosts(vec![b.id], cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(m.hosts().len(), 1);
            assert!(m.host(a.id).unwrap().is_favourite);
            assert_eq!(m.trash().len(), 1);
        });
        let entry = model.read_with(cx, |m, _| m.trash().first().unwrap().id);
        model.update(cx, |m, cx| m.restore(entry, cx));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(m.hosts().len(), 2);
            assert_eq!(m.trash().len(), 0);
        });
    }

    #[gpui::test]
    fn keys_and_identities_save_and_delete_with_a_working_vault(cx: &mut TestAppContext) {
        use ssherlock_core::keychain::{KeyKind, KeyProtection};
        use ssherlock_core::{ExposeSecret as _, SecretString};

        let model = model(cx);
        let vault = model.read_with(cx, |m, _| m.vault().cloned()).unwrap();
        let key = Key {
            id: Id::generate(),
            label: "Laptop".into(),
            kind: KeyKind::Ed25519,
            public_key: "ssh-ed25519 AAAA".into(),
            fingerprint: "SHA256:x".into(),
            private_key: vault.seal(b"private").unwrap(),
            protection: KeyProtection::Unencrypted,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        };
        let mut identity = Identity::new("deploy", Timestamp::UNIX_EPOCH);
        identity.key = Some(key.id);
        identity.password = Some(vault.seal_str(&SecretString::from("pw")).unwrap());
        model.update(cx, |m, cx| {
            m.save_key(key.clone(), cx);
            m.save_identity(identity.clone(), cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(m.key(key.id), Some(&key));
            let saved = m.identity(identity.id).unwrap();
            let password = vault.open_string(saved.password.as_ref().unwrap()).unwrap();
            assert_eq!(password.expose_secret(), "pw");
        });
        model.update(cx, |m, cx| m.delete_key(key.id, cx));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(m.keys().len(), 0);
            assert_eq!(m.identity(identity.id).unwrap().key, None);
        });
    }

    #[gpui::test]
    fn a_failed_change_shows_a_toast_and_keeps_the_data(cx: &mut TestAppContext) {
        let model = model(cx);
        let mut bad = host("example.com");
        bad.label = "two\nlines".into();
        model.update(cx, |m, cx| m.save_hosts(vec![bad], cx));
        cx.run_until_parked();
        assert_eq!(model.read_with(cx, |m, _| m.hosts().len()), 0);
        let toasts = cx.update(|cx| notify::notifications(cx).unwrap().read(cx).toasts().len());
        assert_eq!(toasts, 1);
    }

    #[gpui::test]
    fn an_unopenable_database_says_why(cx: &mut TestAppContext) {
        cx.update(notify::init);
        // A path under a regular file can't be created.
        let file =
            std::env::temp_dir().join(format!("ssherlock-ui-model-{}", Id::<()>::generate()));
        std::fs::write(&file, b"").unwrap();
        let model =
            cx.update(|cx| HostsModel::open(file.join("db.sqlite"), file.join("vault.key"), cx));
        cx.run_until_parked();
        let state = model.read_with(cx, |m, _| m.state().clone());
        let _ = std::fs::remove_file(&file);
        assert!(
            matches!(&state, LoadState::Failed(text) if text.starts_with("could not create the directory")),
            "{state:?}"
        );
    }
}
