//! [`ForwardsModel`]: the port forwards running now, and the forwarding-only connections that
//! carry them (FWD-05…07).
//!
//! One per app. A rule started from Port forwarding runs over a forwarding-only connection to
//! its host ([`spawn_forwarding`]), opened on demand and closed when its last forward stops.
//! Rules set to start on connect run over a terminal tab's own connection instead, and end with
//! the tab. Either way the forward follows its connection through reconnects (`ssherlock-ssh`
//! restarts it). Nothing here is saved: rules are saved with their host, and what's running is
//! forgotten when the app quits.

use std::collections::{HashMap, VecDeque};

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use ssherlock_core::Id;
use ssherlock_core::forwarding::ForwardRule;
use ssherlock_core::hosts::Host;
use ssherlock_ssh::connection::DisconnectReason;
use ssherlock_ssh::{
    ConnectionHandle, ConnectionState, ForwardStats, ForwardStatus, RunningForward, SessionEvent,
    spawn_forwarding,
};
use tokio::sync::mpsc;

use crate::hosts::{ConnectRequest, HostsModel};
use crate::notify;
use crate::ssh::{EVENT_CAPACITY, prepare_connection};

/// Which connection carries a forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carrier {
    /// A terminal tab's connection; the forward started with the tab (FWD-05) and ends with it.
    Tab,
    /// A forwarding-only connection, started from Port forwarding (FWD-06).
    Tunnel,
}

/// A running (or failed) forward, as Port forwarding shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardView {
    /// Where it is.
    pub status: ForwardStatus,
    /// Its traffic.
    pub stats: ForwardStats,
    /// What carries it.
    pub carrier: Carrier,
}

struct Active {
    rule: Id<ForwardRule>,
    host: Id<Host>,
    carrier: Carrier,
    running: RunningForward,
    _watch: Task<()>,
}

/// A forwarding-only connection to one host.
struct Tunnel {
    connection: ConnectionHandle,
    label: SharedString,
    state: Option<ConnectionState>,
    _events: Task<()>,
}

/// The forwards running now, and how to start and stop them.
#[derive(Default)]
pub struct ForwardsModel {
    active: Vec<Active>,
    tunnels: HashMap<Id<Host>, Tunnel>,
    /// Questions forwarding-only connections are waiting on (host keys, passwords), oldest
    /// first, for Port forwarding to ask.
    questions: VecDeque<(Id<Host>, SessionEvent)>,
}

struct GlobalForwardsModel(Entity<ForwardsModel>);

impl Global for GlobalForwardsModel {}

impl ForwardsModel {
    /// The app's model, if [`install`](Self::install) has run.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalForwardsModel>().map(|g| g.0.clone())
    }

    /// Installs an empty model as the app's.
    pub fn install(cx: &mut App) -> Entity<Self> {
        let model = cx.new(|_| Self::default());
        cx.set_global(GlobalForwardsModel(model.clone()));
        model
    }

    /// How `rule` is doing, if it's running or failed to.
    pub fn view(&self, rule: Id<ForwardRule>) -> Option<ForwardView> {
        self.active
            .iter()
            .find(|a| a.rule == rule)
            .map(|a| ForwardView {
                status: a.running.status(),
                stats: a.running.stats(),
                carrier: a.carrier,
            })
    }

    /// Whether `rule` is running (not stopped or failed).
    pub fn is_running(&self, rule: Id<ForwardRule>) -> bool {
        self.active
            .iter()
            .any(|a| a.rule == rule && !a.running.status().is_finished())
    }

    /// Whether any forward is running, so the view knows to keep its counters fresh.
    pub fn has_running(&self) -> bool {
        self.active
            .iter()
            .any(|a| !a.running.status().is_finished())
    }

    /// The forwarding-only connection to `host`, if one is open: its label and latest state.
    pub fn tunnel(&self, host: Id<Host>) -> Option<(&SharedString, Option<&ConnectionState>)> {
        self.tunnels
            .get(&host)
            .map(|t| (&t.label, t.state.as_ref()))
    }

    /// The oldest question a forwarding-only connection is waiting on, taken off the queue.
    pub fn take_question(&mut self) -> Option<(Id<Host>, SessionEvent)> {
        self.questions.pop_front()
    }

    /// The label of `host`'s forwarding-only connection, for a dialog's context.
    pub fn tunnel_label(&self, host: Id<Host>) -> Option<SharedString> {
        self.tunnels.get(&host).map(|t| t.label.clone())
    }

    /// Starts `rule` from Port forwarding, over a forwarding-only connection to its host,
    /// opening one if needed. A rule already running, or disabled, isn't started.
    pub fn start(&mut self, rule: &ForwardRule, cx: &mut Context<Self>) {
        if !rule.is_enabled || self.is_running(rule.id) {
            return;
        }
        let Some(connection) = self.tunnel_to(rule.host, cx) else {
            return;
        };
        self.remove(rule.id);
        self.launch(rule, &connection, Carrier::Tunnel, cx);
    }

    /// Starts the enabled rules of `host` that start on connect (FWD-05), over a terminal
    /// tab's `connection`. Rules already running elsewhere are left alone: their ports are
    /// taken.
    pub fn start_auto(
        &mut self,
        host: Id<Host>,
        connection: &ConnectionHandle,
        cx: &mut Context<Self>,
    ) {
        let Some(hosts) = HostsModel::global(cx) else {
            return;
        };
        let rules: Vec<ForwardRule> = hosts
            .read(cx)
            .forward_rules_of(host)
            .filter(|r| r.is_enabled && r.auto_start)
            .cloned()
            .collect();
        for rule in rules {
            if !self.is_running(rule.id) {
                self.remove(rule.id);
                self.launch(&rule, connection, Carrier::Tab, cx);
            }
        }
    }

    /// Stops `rule` (or forgets its failure). A forwarding-only connection left with nothing
    /// to carry is closed.
    pub fn stop(&mut self, rule: Id<ForwardRule>, cx: &mut Context<Self>) {
        let host = self.active.iter().find(|a| a.rule == rule).map(|a| a.host);
        self.remove(rule);
        if let Some(host) = host {
            self.close_idle_tunnel(host);
        }
        cx.notify();
    }

    /// Stops every running forward.
    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        self.active.clear();
        self.tunnels.clear();
        self.questions.clear();
        cx.notify();
    }

    fn remove(&mut self, rule: Id<ForwardRule>) {
        // Dropping the running forward stops it.
        self.active.retain(|a| a.rule != rule);
    }

    /// Closes `host`'s forwarding-only connection if no running forward uses it.
    fn close_idle_tunnel(&mut self, host: Id<Host>) {
        let in_use = self.active.iter().any(|a| {
            a.host == host && a.carrier == Carrier::Tunnel && !a.running.status().is_finished()
        });
        if !in_use && self.tunnels.remove(&host).is_some() {
            self.questions.retain(|(h, _)| *h != host);
            tracing::info!("closed a forwarding-only connection with nothing left to carry");
        }
    }

    /// Starts `rule` on `connection` and follows its status.
    fn launch(
        &mut self,
        rule: &ForwardRule,
        connection: &ConnectionHandle,
        carrier: Carrier,
        cx: &mut Context<Self>,
    ) {
        let running = connection.start_forward(rule.forward.clone());
        let mut watch = running.watch();
        let id = rule.id;
        let watcher = cx.spawn(async move |this, cx| {
            loop {
                let finished = watch.borrow_and_update().clone();
                if finished.is_finished() {
                    let _ = this.update(cx, |model, cx| model.finished(id, &finished, cx));
                    return;
                }
                let _ = this.update(cx, |_, cx| cx.notify());
                if watch.changed().await.is_err() {
                    return;
                }
            }
        });
        self.active.push(Active {
            rule: rule.id,
            host: rule.host,
            carrier,
            running,
            _watch: watcher,
        });
        cx.notify();
    }

    /// A forward ended. A stopped one is forgotten; a failed one stays, so Port forwarding can
    /// say why, until it's started again or dismissed.
    fn finished(&mut self, rule: Id<ForwardRule>, status: &ForwardStatus, cx: &mut Context<Self>) {
        let host = self.active.iter().find(|a| a.rule == rule).map(|a| a.host);
        if *status == ForwardStatus::Stopped {
            self.remove(rule);
        }
        if let Some(host) = host {
            self.close_idle_tunnel(host);
        }
        cx.notify();
    }

    /// The forwarding-only connection to `host`, opened if there isn't one. `None`, after
    /// saying why, if it can't be opened.
    fn tunnel_to(&mut self, host: Id<Host>, cx: &mut Context<Self>) -> Option<ConnectionHandle> {
        if let Some(tunnel) = self.tunnels.get(&host) {
            return Some(tunnel.connection.clone());
        }
        let hosts = HostsModel::global(cx)?;
        let request = {
            let model = hosts.read(cx);
            let saved = model.host(host)?;
            ConnectRequest::new(saved, model.groups(), model.identities())
        };
        let prepared = prepare_connection(&request.target, &request.options, cx);
        let setup = match prepared.setup {
            Ok(setup) => setup,
            Err(reason) => {
                notify::info(
                    format!(
                        "Port forwarding to {} can't start: {reason}",
                        prepared.label
                    ),
                    cx,
                );
                return None;
            }
        };
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        let connection = spawn_forwarding(setup.config, setup.host_keys, tx, &setup.runtime);
        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                if this
                    .update(cx, |model, cx| model.tunnel_event(host, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        tracing::info!(host = %prepared.label, "opening a forwarding-only connection");
        self.tunnels.insert(
            host,
            Tunnel {
                connection: connection.clone(),
                label: prepared.label,
                state: None,
                _events: events,
            },
        );
        hosts.update(cx, |model, cx| model.record_connection(host, cx));
        Some(connection)
    }

    fn tunnel_event(&mut self, host: Id<Host>, event: SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::State(state) => {
                if let ConnectionState::Disconnected(reason) = &state
                    && let Some(tunnel) = self.tunnels.remove(&host)
                {
                    if let Some(why) = disconnect_text(reason) {
                        notify::info(
                            format!("Port forwarding to {} stopped: {why}", tunnel.label),
                            cx,
                        );
                    }
                    self.questions.retain(|(h, _)| *h != host);
                } else if let Some(tunnel) = self.tunnels.get_mut(&host) {
                    tunnel.state = Some(state);
                }
            }
            SessionEvent::Banner(banner) => {
                tracing::info!(banner = banner.trim(), "server banner");
            }
            question @ (SessionEvent::HostKey(_) | SessionEvent::Prompt(_)) => {
                self.questions.push_back((host, question));
            }
        }
        cx.notify();
    }
}

/// Why a forwarding-only connection finished, for a toast; `None` when it was simply closed.
fn disconnect_text(reason: &DisconnectReason) -> Option<String> {
    match reason {
        DisconnectReason::Closed | DisconnectReason::ShellExited(_) => None,
        DisconnectReason::Failed(message) => Some(message.clone()),
        DisconnectReason::GaveUp { attempts } => Some(format!(
            "the connection was lost and {attempts} attempts to reconnect failed"
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use ssherlock_core::forwarding::{Forward, Listen};
    use ssherlock_core::{Port, Timestamp};
    use ssherlock_ssh::session::HostKeyDecision;
    use ssherlock_store::{Repository as _, Store};

    use super::*;
    use crate::forwarding::test_support;

    #[gpui::test]
    fn a_tab_starts_only_its_hosts_enabled_rules_that_start_on_connect(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (ssh_port, _) = runtime.block_on(test_support::servers());
        let free = || {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        };
        let mut host = Host::new("127.0.0.1".parse().unwrap(), Timestamp::UNIX_EPOCH);
        host.settings.port = Port::new(ssh_port);
        let proxy = |port: u16| {
            ForwardRule::new(
                host.id,
                Forward::Dynamic {
                    listen: Listen::localhost(Port::new(port).unwrap()),
                },
                Timestamp::UNIX_EPOCH,
            )
        };
        let mut auto = proxy(free());
        auto.auto_start = true;
        let manual = proxy(free());
        let mut disabled = proxy(free());
        disabled.auto_start = true;
        disabled.is_enabled = false;
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.upsert(&host).unwrap();
        store
            .upsert_all(&[auto.clone(), manual.clone(), disabled.clone()])
            .unwrap();
        let dir = std::env::temp_dir().join(format!("ssherlock-auto-{}", Id::<()>::generate()));
        cx.update(|cx| {
            crate::notify::init(cx);
            cx.set_global(crate::runtime::TokioHandle::new(runtime.handle().clone()));
            cx.set_global(crate::ssh::SshEnvironment {
                known_hosts: dir.join("known_hosts"),
                read_only_known_hosts: Vec::new(),
                ssh_dir: None,
                local_user: Some("tester".into()),
            });
            HostsModel::install(store, cx);
        });
        let forwards = cx.update(ForwardsModel::install);
        cx.run_until_parked();

        // The connection a terminal tab would own, with its host key trusted as the tab's
        // dialog would.
        let target = ssherlock_core::ConnectTarget {
            user: Some("tester".into()),
            address: host.address.clone(),
            port: host.settings.port,
        };
        let setup = cx
            .update(|cx| prepare_connection(&target, &crate::ssh::ConnectOptions::default(), cx))
            .setup
            .unwrap();
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        runtime.spawn(async move {
            while let Some(event) = rx.recv().await {
                if let SessionEvent::HostKey(prompt) = event {
                    prompt.answer(HostKeyDecision::Trust);
                }
            }
        });
        let connection = spawn_forwarding(setup.config, setup.host_keys, tx, &setup.runtime);
        forwards.update(cx, |model, cx| model.start_auto(host.id, &connection, cx));

        let (window, cx) = cx.add_window_view(|_, _| gpui::Empty);
        drop(window);
        test_support::wait_until(cx, "the auto-start rule to run", |cx| {
            forwards.read_with(cx, |f, _| {
                f.view(auto.id)
                    .is_some_and(|v| v.status == ForwardStatus::Active)
            })
        });
        forwards.read_with(cx, |f, _| {
            assert_eq!(f.view(auto.id).map(|v| v.carrier), Some(Carrier::Tab));
            assert!(f.view(manual.id).is_none() && f.view(disabled.id).is_none());
            // A tab's forwards don't open a forwarding-only connection.
            assert!(f.tunnel(host.id).is_none());
        });

        // The tab closing ends its connection, and the forward with it.
        drop(connection);
        test_support::wait_until(cx, "the forward to end with the tab", |cx| {
            forwards.read_with(cx, |f, _| f.view(auto.id).is_none())
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disconnects_are_explained() {
        assert_eq!(disconnect_text(&DisconnectReason::Closed), None);
        assert_eq!(
            disconnect_text(&DisconnectReason::Failed("could not connect".into())).as_deref(),
            Some("could not connect")
        );
        assert_eq!(
            disconnect_text(&DisconnectReason::GaveUp { attempts: 10 }).as_deref(),
            Some("the connection was lost and 10 attempts to reconnect failed")
        );
    }
}
