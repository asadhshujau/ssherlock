//! [`HostsModel`]: the saved hosts, groups and trash, shared by every view that shows them.
//!
//! One per app. It owns the [`Store`] and keeps an in-memory copy of what's in it, so views
//! render from memory and never touch the database. Every change goes to the store on a
//! background thread; changes run one at a time, in the order they were asked for, and each
//! ends by reloading, so the copy always matches the database. Failures become toasts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use ssherlock_core::hosts::{Group, Host};
use ssherlock_core::trash::TrashEntry;
use ssherlock_core::{Id, Timestamp};
use ssherlock_store::{Repository, Store, StoreError};

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

/// Everything the hosts views show, read in one go.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    hosts: Vec<Host>,
    groups: Vec<Group>,
    trash: Vec<TrashEntry>,
}

impl Snapshot {
    fn load(store: &Store) -> Result<Self, StoreError> {
        let mut trash: Vec<TrashEntry> = store.list()?;
        trash.sort_by_key(|entry| std::cmp::Reverse(entry.deleted_at));
        Ok(Self {
            hosts: store.list()?,
            groups: store.list()?,
            trash,
        })
    }
}

/// The saved hosts, groups and trash, and the way to change them.
pub struct HostsModel {
    store: Option<Arc<Store>>,
    state: LoadState,
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

    /// Opens the database at `path` in the background and installs the model as the app's.
    pub fn open(path: PathBuf, cx: &mut App) -> Entity<Self> {
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
                let _ = this.update(cx, |model: &mut Self, cx| {
                    match opened {
                        Ok((store, snapshot)) => {
                            model.store = Some(store);
                            model.data = snapshot;
                            model.state = LoadState::Ready;
                        }
                        Err(error) => {
                            notify::error(&error, cx);
                            model.state = LoadState::Failed(error_text(&error).into());
                        }
                    }
                    cx.notify();
                });
            }));
            model
        });
        cx.set_global(GlobalHostsModel(model.clone()));
        model
    }

    /// Installs a model over an already-open `store` (tests, or a store opened elsewhere).
    pub fn install(store: Arc<Store>, cx: &mut App) -> Entity<Self> {
        let model = cx.new(|cx| {
            let mut model = Self::empty(LoadState::Loading);
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

    fn empty(state: LoadState) -> Self {
        Self {
            store: None,
            state,
            data: Snapshot::default(),
            queue: None,
            sessions: HashMap::new(),
        }
    }

    /// Whether the hosts are loaded, still loading, or unavailable.
    pub fn state(&self) -> &LoadState {
        &self.state
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
        let model = cx.update(|cx| HostsModel::open(file.join("db.sqlite"), cx));
        cx.run_until_parked();
        let state = model.read_with(cx, |m, _| m.state().clone());
        let _ = std::fs::remove_file(&file);
        assert!(
            matches!(&state, LoadState::Failed(text) if text.starts_with("could not create the directory")),
            "{state:?}"
        );
    }
}
